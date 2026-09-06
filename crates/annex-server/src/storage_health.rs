//! Storage-health gate.
//!
//! The previous behaviour on "disk full" was to let `SQLITE_FULL` /
//! `SQLITE_IOERR` bubble up through `ApiError::InternalServerError` as a
//! generic HTTP 500. That made disk exhaustion indistinguishable from a
//! bug, which is the worst-case operational signal — an admin would
//! reach for the wrong runbook every time.
//!
//! This module holds a single `AtomicU8` representing the storage
//! state. Writes consult it before doing any I/O; reads ignore it.
//! Promotion to `Degraded` happens from two places:
//!
//!   * the background storage probe (`crate::background::start_storage_probe_task`)
//!     compares the DB file size against `Config::storage::block_free_bytes`
//!     and `warn_free_bytes` and flips the gate if appropriate;
//!   * any write site that catches an `SQLITE_FULL` / `SQLITE_IOERR`
//!     and wants the next request to fail-fast rather than retry into
//!     the same error (see [`interpret_sqlite_error`]).
//!
//! There is no automatic recovery: an operator who has freed disk
//! must call `mark_healthy()` (via the admin endpoint) once they have
//! verified the situation. Auto-recovery on a probe success would
//! flap under transient I/O errors and create harder-to-debug
//! "writes work sometimes" symptoms.
//!
//! Cross-platform "free disk bytes" inspection is intentionally NOT
//! attempted here. The portable signal we *do* have is the DB file
//! size (via `std::fs::metadata`) plus reactive detection of
//! `SQLITE_FULL` from the engine itself. That covers the operator
//! scenarios that matter (db growing past a configured cap; SQLite
//! reporting it could not write) without adding `libc` /
//! `windows_sys` to the dependency set.

use std::sync::atomic::{AtomicU8, Ordering};

const HEALTHY: u8 = 0;
const WARN: u8 = 1;
const DEGRADED: u8 = 2;

/// Current storage state. Held inside `AppState` as an `Arc<StorageHealth>`.
#[derive(Debug)]
pub struct StorageHealth {
    state: AtomicU8,
    /// Reason the gate is closed, recorded for diagnostics. Empty
    /// while healthy.
    reason: std::sync::RwLock<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageState {
    Healthy,
    Warn,
    Degraded,
}

impl StorageState {
    pub fn as_str(self) -> &'static str {
        match self {
            StorageState::Healthy => "healthy",
            StorageState::Warn => "warn",
            StorageState::Degraded => "degraded",
        }
    }
}

impl StorageHealth {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(HEALTHY),
            reason: std::sync::RwLock::new(String::new()),
        }
    }

    pub fn state(&self) -> StorageState {
        match self.state.load(Ordering::Acquire) {
            HEALTHY => StorageState::Healthy,
            WARN => StorageState::Warn,
            _ => StorageState::Degraded,
        }
    }

    pub fn reason(&self) -> String {
        self.reason
            .read()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    /// True when the gate is closed for writes.
    pub fn writes_blocked(&self) -> bool {
        self.state() == StorageState::Degraded
    }

    pub fn mark_warn(&self, reason: impl Into<String>) {
        // Don't downgrade from Degraded to Warn — Degraded is
        // operator-cleared.
        let cur = self.state.load(Ordering::Acquire);
        if cur < WARN {
            self.state.store(WARN, Ordering::Release);
        }
        if let Ok(mut r) = self.reason.write() {
            *r = reason.into();
        }
    }

    pub fn mark_degraded(&self, reason: impl Into<String>) {
        self.state.store(DEGRADED, Ordering::Release);
        if let Ok(mut r) = self.reason.write() {
            *r = reason.into();
        }
    }

    pub fn mark_healthy(&self) {
        self.state.store(HEALTHY, Ordering::Release);
        if let Ok(mut r) = self.reason.write() {
            r.clear();
        }
    }
}

impl Default for StorageHealth {
    fn default() -> Self {
        Self::new()
    }
}

/// Inspect a `rusqlite::Error` and, if it represents an out-of-space
/// or general I/O failure that should keep happening, trip the gate.
/// Returns the same error so callers can `?` through it. Idempotent
/// on already-degraded gates.
pub fn interpret_sqlite_error(health: &StorageHealth, e: &rusqlite::Error) -> bool {
    let trip = matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if matches!(
                err.code,
                rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::SystemIoFailure
            )
    );
    if trip {
        health.mark_degraded(format!("sqlite error trip: {e}"));
    }
    trip
}

/// Inspect the database's on-disk size — the main file plus its WAL
/// sidecars — against the operator's thresholds and
/// flip the gate accordingly. The `block_free_bytes` and
/// `warn_free_bytes` config values are interpreted here as
/// "headroom" — i.e. "the DB file may grow until it is within
/// `block_free_bytes` of the configured max, then writes are blocked."
/// In the absence of a max-size config we treat the gate as healthy.
///
/// Returns the current `StorageState` for logging.
pub fn evaluate_db_file_size(
    health: &StorageHealth,
    db_path: &std::path::Path,
    warn_free_bytes: u64,
    block_free_bytes: u64,
    max_bytes: Option<u64>,
) -> StorageState {
    let main = match std::fs::metadata(db_path) {
        Ok(m) => m.len(),
        Err(_) => return health.state(),
    };
    // Plus the WAL sidecars. The pool opens the database in WAL mode, so
    // between checkpoints `-wal` can reach hundreds of megabytes — on the
    // same filesystem, counting against the same disk. A cap that measured
    // only the main file would let that fill the disk while the gate went
    // on reporting healthy, which is the exact failure this module exists
    // to get ahead of. Absent sidecars contribute nothing, so a non-WAL
    // database reads the same as before.
    let sidecars: u64 = ["-wal", "-shm"]
        .iter()
        .filter_map(|suffix| {
            let mut name = db_path.as_os_str().to_os_string();
            name.push(suffix);
            std::fs::metadata(std::path::PathBuf::from(name)).ok()
        })
        .map(|m| m.len())
        .sum();
    let size = main.saturating_add(sidecars);

    let max = match max_bytes {
        Some(m) if m > 0 => m,
        // No cap configured → only the reactive trip path can move us
        // out of Healthy. Leave state unchanged.
        _ => return health.state(),
    };

    let headroom = max.saturating_sub(size);
    if headroom <= block_free_bytes {
        health.mark_degraded(format!(
            "db file size {size} bytes is within {block_free_bytes} of cap {max}"
        ));
    } else if headroom <= warn_free_bytes {
        health.mark_warn(format!(
            "db file size {size} bytes is within {warn_free_bytes} of cap {max}"
        ));
    }
    health.state()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_health_is_healthy() {
        let h = StorageHealth::new();
        assert_eq!(h.state(), StorageState::Healthy);
        assert!(!h.writes_blocked());
        assert_eq!(h.reason(), "");
    }

    #[test]
    fn warn_records_reason_but_does_not_block() {
        let h = StorageHealth::new();
        h.mark_warn("free space below warn threshold");
        assert_eq!(h.state(), StorageState::Warn);
        assert!(!h.writes_blocked());
        assert_eq!(h.reason(), "free space below warn threshold");
    }

    #[test]
    fn degraded_blocks_writes() {
        let h = StorageHealth::new();
        h.mark_degraded("free space below block threshold");
        assert_eq!(h.state(), StorageState::Degraded);
        assert!(h.writes_blocked());
    }

    #[test]
    fn warn_does_not_downgrade_degraded() {
        let h = StorageHealth::new();
        h.mark_degraded("disk full");
        h.mark_warn("free space recovered to warn level");
        assert_eq!(
            h.state(),
            StorageState::Degraded,
            "WARN must NOT silently clear DEGRADED — operator must explicitly call mark_healthy"
        );
    }

    #[test]
    fn mark_healthy_clears_state_and_reason() {
        let h = StorageHealth::new();
        h.mark_degraded("disk full");
        h.mark_healthy();
        assert_eq!(h.state(), StorageState::Healthy);
        assert_eq!(h.reason(), "");
    }

    /// The WAL is on the same filesystem and counts against the same
    /// disk, so a cap that ignored it would be measuring the wrong thing.
    #[test]
    fn evaluate_db_file_size_counts_the_wal_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("annex.db");
        std::fs::write(&db, vec![0u8; 600]).unwrap();

        // 600 bytes against a 1000-byte cap leaves 400 of headroom, which
        // clears a 200-byte blocking threshold.
        let h = StorageHealth::new();
        assert_eq!(
            evaluate_db_file_size(&h, &db, 400, 200, Some(1000)),
            StorageState::Warn
        );

        // The same database with a 300-byte WAL occupies 900, leaving 100
        // — inside the blocking threshold.
        std::fs::write(dir.path().join("annex.db-wal"), vec![0u8; 300]).unwrap();
        let h = StorageHealth::new();
        assert_eq!(
            evaluate_db_file_size(&h, &db, 400, 200, Some(1000)),
            StorageState::Degraded
        );
        assert!(h.writes_blocked());
    }

    #[test]
    fn evaluate_db_file_size_no_cap_leaves_healthy() {
        let h = StorageHealth::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let s = evaluate_db_file_size(&h, tmp.path(), 1024, 256, None);
        assert_eq!(s, StorageState::Healthy);
    }

    #[test]
    fn evaluate_db_file_size_within_block_threshold_degrades() {
        let h = StorageHealth::new();
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        // Write 900 bytes.
        tmp.write_all(&vec![b'x'; 900]).unwrap();
        // Cap = 1000, block_free = 200 → headroom (1000-900=100) ≤ 200 → Degraded.
        let s = evaluate_db_file_size(&h, tmp.path(), 400, 200, Some(1000));
        assert_eq!(s, StorageState::Degraded);
    }

    #[test]
    fn evaluate_db_file_size_within_warn_threshold_warns() {
        let h = StorageHealth::new();
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        // Write 600 bytes.
        tmp.write_all(&vec![b'x'; 600]).unwrap();
        // Cap = 1000, warn = 500, block = 100 → headroom 400 ≤ warn 500 → Warn.
        let s = evaluate_db_file_size(&h, tmp.path(), 500, 100, Some(1000));
        assert_eq!(s, StorageState::Warn);
    }

    #[test]
    fn interpret_sqlite_error_trips_on_disk_full() {
        let h = StorageHealth::new();
        let err = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            None,
        );
        assert!(interpret_sqlite_error(&h, &err));
        assert_eq!(h.state(), StorageState::Degraded);
    }

    #[test]
    fn interpret_sqlite_error_ignores_constraint_violations() {
        let h = StorageHealth::new();
        let err = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE),
            None,
        );
        assert!(!interpret_sqlite_error(&h, &err));
        assert_eq!(h.state(), StorageState::Healthy);
    }
}
