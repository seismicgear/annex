//! Application data directory + on-disk `config.toml` bootstrapping.
//!
//! The desktop app writes a default config the first time it starts, then
//! delegates to `annex_server::config::load_config` for actual parsing.
//! This module intentionally only knows about the file layout — schema
//! changes belong in the server crate.

use std::path::{Path, PathBuf};

/// Resolve the application data directory.
///
/// Uses `dirs::data_dir()` to locate the platform-specific directory:
/// - Windows: `%APPDATA%\Annex`
/// - macOS: `~/Library/Application Support/Annex`
/// - Linux: `~/.local/share/Annex`
pub(crate) fn resolve_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Annex")
}

/// Writes a default `config.toml` into the data directory if one does not
/// already exist, and ensures any Windows backslash paths are corrected.
///
/// Returns the path to the config file on success. Returns an error if the
/// config file cannot be created or a backslash migration cannot be persisted.
pub(crate) fn ensure_config(data_dir: &Path) -> Result<PathBuf, String> {
    let config_path = data_dir.join("config.toml");
    if !config_path.exists() {
        let db_path = data_dir.join("annex.db");
        let upload_dir = data_dir.join("uploads");
        // Use forward slashes for the database path — Windows APIs accept
        // them, and TOML double-quoted strings treat backslashes as escape
        // sequences (e.g. \U → unicode escape), which breaks parsing.
        let db_path_safe = db_path.display().to_string().replace('\\', "/");
        let contents = format!(
            r#"# Annex desktop configuration (auto-generated).

[server]
host = "127.0.0.1"
port = 0

[database]
path = "{db_path_safe}"
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = "info"
json = false

[cors]
# Desktop defaults: allow Tauri webview origins (macOS/Linux + Windows).
# Override with ANNEX_CORS_ORIGINS env var if needed.
allowed_origins = ["tauri://localhost", "https://tauri.localhost", "http://tauri.localhost"]

# [webrtc]
# Uncomment and configure to enable voice channels (WebRTC).
# url = "ws://localhost:7880"
# api_key = ""
# api_secret = ""
# token_ttl_seconds = 3600
#
# STUN/TURN servers for WebRTC NAT traversal. Defaults to Google STUN.
# Add TURN servers for restrictive corporate networks that block UDP.
# [[webrtc.ice_servers]]
# urls = ["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"]
#
# [[webrtc.ice_servers]]
# urls = ["turn:turn.example.com:3478?transport=udp", "turns:turn.example.com:5349?transport=tcp"]
# username = "your-turn-username"
# credential = "your-turn-credential"
"#,
        );
        std::fs::write(&config_path, contents).map_err(|e| {
            format!(
                "failed to write default config to {}: {e}",
                config_path.display()
            )
        })?;

        // Pre-create the upload directory (non-fatal if this fails).
        let _ = std::fs::create_dir_all(&upload_dir);
    }

    // Always fix backslash paths regardless of whether the config was just
    // created or already existed. This handles configs from older versions
    // that wrote Windows-style paths, and acts as a safety net in case the
    // forward-slash replacement above is ever bypassed.
    fix_backslash_paths(&config_path)?;

    Ok(config_path)
}

/// Replaces Windows backslashes with forward slashes in a config file.
///
/// TOML double-quoted strings treat `\U` as an 8-digit unicode escape, so a
/// path like `C:\Users\monty\AppData\...\annex.db` fails to parse. This
/// function detects the drive-letter pattern `:\` and replaces all backslashes
/// with forward slashes, which Windows APIs accept.
pub(crate) fn fix_backslash_paths(config_path: &Path) -> Result<(), String> {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "failed to read config at {}: {e}",
                config_path.display()
            ))
        }
    };

    if contents.contains(":\\") {
        let fixed = contents.replace('\\', "/");
        std::fs::write(config_path, fixed).map_err(|e| {
            format!(
                "failed to fix backslash paths in config {}: {e}",
                config_path.display()
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_config_creates_file_with_all_sections() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        assert!(config_path.exists(), "config file must be created");

        let contents = std::fs::read_to_string(&config_path).expect("should read config");

        // Verify all expected sections are present
        assert!(contents.contains("[server]"), "missing [server] section");
        assert!(
            contents.contains("[database]"),
            "missing [database] section"
        );
        assert!(contents.contains("[logging]"), "missing [logging] section");
        assert!(contents.contains("[cors]"), "missing [cors] section");

        // Verify the webrtc comment block is present
        assert!(
            contents.contains("# [webrtc]"),
            "missing commented [webrtc] section"
        );
        assert!(
            contents.contains("# url = \"ws://localhost:7880\""),
            "missing commented webrtc url"
        );
        assert!(
            contents.contains("# api_key = \"\""),
            "missing commented webrtc api_key"
        );
        assert!(
            contents.contains("# api_secret = \"\""),
            "missing commented webrtc api_secret"
        );
        assert!(
            contents.contains("# token_ttl_seconds = 3600"),
            "missing commented webrtc token_ttl_seconds"
        );
    }

    #[test]
    fn ensure_config_is_valid_toml_with_voice_dev_defaults() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let config_path_str = config_path.to_string_lossy();

        // The file should parse cleanly via the server config loader.
        // Since the [webrtc] section is fully commented out, the TOML parser
        // should see no webrtc fields and use WebRtcConfig::default(),
        // which now contains dev server values so voice works out of the box.
        let cfg =
            annex_server::config::load_config(Some(&config_path_str)).expect("config should parse");

        // Voice defaults to dev configuration (auto-start WebRTC)
        assert_eq!(
            cfg.webrtc.url,
            annex_voice::DEV_WEBRTC_URL,
            "webrtc.url should default to dev URL"
        );
        assert_eq!(
            cfg.webrtc.api_key,
            annex_voice::DEV_WEBRTC_API_KEY,
            "webrtc.api_key should default to dev key"
        );
        assert_eq!(
            cfg.webrtc.api_secret,
            annex_voice::DEV_WEBRTC_API_SECRET,
            "webrtc.api_secret should default to dev secret"
        );
        assert_eq!(
            cfg.webrtc.token_ttl_seconds, 3600,
            "webrtc.token_ttl_seconds should default to 3600"
        );
    }

    #[test]
    fn ensure_config_is_idempotent() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");

        // First call creates the file
        let path1 = ensure_config(dir.path()).expect("first call should succeed");
        let contents1 = std::fs::read_to_string(&path1).expect("should read");

        // Second call should not overwrite
        let path2 = ensure_config(dir.path()).expect("second call should succeed");
        let contents2 = std::fs::read_to_string(&path2).expect("should read");

        assert_eq!(path1, path2, "paths should match");
        assert_eq!(
            contents1, contents2,
            "contents should not change on second call"
        );
    }

    #[test]
    fn ensure_config_creates_db_path_in_data_dir() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let contents = std::fs::read_to_string(&config_path).expect("should read");

        // Database path should point to the data directory
        let expected_db = dir.path().join("annex.db");
        let expected_db_safe = expected_db.display().to_string().replace('\\', "/");
        assert!(
            contents.contains(&expected_db_safe),
            "config should contain db path: {expected_db_safe}"
        );
    }

    #[test]
    fn fix_backslash_paths_is_noop_for_clean_config() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let before = std::fs::read_to_string(&config_path).expect("should read");

        fix_backslash_paths(&config_path).expect("fix should succeed");

        let after = std::fs::read_to_string(&config_path).expect("should read");
        assert_eq!(before, after, "clean config should not be modified");
    }

    #[test]
    fn config_template_documents_turn_servers() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let contents = std::fs::read_to_string(&config_path).expect("should read config");

        assert!(
            contents.contains("[[webrtc.ice_servers]]"),
            "config template should document ICE server configuration"
        );
        assert!(
            contents.contains("turn:"),
            "config template should include TURN server example"
        );
        assert!(
            contents.contains("stun:stun.l.google.com"),
            "config template should document default STUN server"
        );
    }
}
