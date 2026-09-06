//! Every write transaction in this workspace must be `BEGIN IMMEDIATE`.
//!
//! Under WAL, a DEFERRED transaction that reads before it writes takes a
//! snapshot and then has to upgrade. A concurrent commit turns that upgrade
//! into `SQLITE_BUSY_SNAPSHOT` *immediately*, with the busy handler never
//! invoked — so `busy_timeout` cannot help, and it surfaces as an
//! intermittent "database is locked" on an ordinary operation.
//!
//! This has regressed three times: first in `edit_message`/`delete_message`,
//! then in `send_message` a release later, then across eleven more sites. The
//! rule was written down in CLAUDE.md each time and re-broken anyway, because
//! nothing checked it. This is the check.
//!
//! It reads source rather than exercising behaviour on purpose: the failure it
//! guards against is a *race*, so a behavioural test for it is either slow and
//! flaky or passes for the wrong reason. `tests/ws_send_immediate_tx.rs`
//! covers one site behaviourally; this covers all of them cheaply.

use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("annex-server lives under crates/")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Only the crates' own sources; `target/` is generated and
            // `tests/` may legitimately construct a deferred transaction to
            // reproduce the very contention being guarded against.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == "node_modules" || name == "tests" {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Strip `//` line comments so a rule written *about* the hazard is not read
/// as an instance of it — several sites carry a comment naming
/// `unchecked_transaction()` as the thing they replaced.
fn without_line_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_write_transaction_is_immediate() {
    let mut files = Vec::new();
    rust_sources(&crates_dir(), &mut files);
    assert!(
        files.len() > 50,
        "expected to find the workspace sources, found {} files — has the \
         layout moved?",
        files.len()
    );

    let mut offenders: Vec<String> = Vec::new();

    for file in &files {
        let Ok(raw) = std::fs::read_to_string(file) else {
            continue;
        };
        let src = without_line_comments(&raw);

        for (i, line) in src.lines().enumerate() {
            let n = i + 1;

            // `conn.transaction()` and `conn.unchecked_transaction()` are both
            // DEFERRED.
            if line.contains(".transaction()") || line.contains(".unchecked_transaction()") {
                offenders.push(format!(
                    "{}:{n}: DEFERRED transaction — use \
                     `transaction_with_behavior(TransactionBehavior::Immediate)`",
                    file.display()
                ));
            }

            // The behaviour argument may wrap onto the following lines.
            if line.contains("transaction_with_behavior") {
                let window: String = src.lines().skip(i).take(4).collect::<Vec<_>>().join(" ");
                if !window.contains("Immediate") {
                    offenders.push(format!(
                        "{}:{n}: transaction_with_behavior without Immediate",
                        file.display()
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "write transactions must be IMMEDIATE under WAL:\n  {}",
        offenders.join("\n  ")
    );
}
