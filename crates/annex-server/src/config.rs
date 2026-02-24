//! Server configuration loading from file and environment variables.

use annex_voice::LiveKitConfig;
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use thiserror::Error;

/// Top-level server configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Server network settings.
    #[serde(default)]
    pub server: ServerConfig,

    /// Database settings.
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Logging settings.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// LiveKit configuration.
    #[serde(default)]
    pub livekit: LiveKitConfig,

    /// Voice pipeline paths (TTS binary, STT model, etc.).
    #[serde(default)]
    pub voice: VoicePathsConfig,

    /// CORS configuration.
    #[serde(default)]
    pub cors: CorsConfig,

    /// Security enforcement settings.
    #[serde(default)]
    pub security: SecurityConfig,
}

/// Security enforcement configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecurityConfig {
    /// When true, channel access endpoints require a valid ZK membership proof
    /// via the `x-annex-zk-proof` header. Default: false (backward-compatible).
    #[serde(default)]
    pub enforce_zk_proofs: bool,
}

/// CORS (Cross-Origin Resource Sharing) configuration.
///
/// By default, CORS is **restrictive** (same-origin only). To allow cross-origin
/// requests, set `allowed_origins` to a list of origin URLs or `["*"]` for
/// permissive mode.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CorsConfig {
    /// List of allowed origins. Empty = same-origin only. `["*"]` = allow all.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

/// File-system paths for the TTS and STT voice pipelines.
#[derive(Debug, Clone, Deserialize)]
pub struct VoicePathsConfig {
    /// Directory containing Piper voice model files.
    #[serde(default = "default_tts_voices_dir")]
    pub tts_voices_dir: String,

    /// Path to the Piper TTS binary.
    #[serde(default = "default_tts_binary_path")]
    pub tts_binary_path: String,

    /// Path to the Whisper GGML model file.
    #[serde(default = "default_stt_model_path")]
    pub stt_model_path: String,

    /// Path to the Whisper STT binary.
    #[serde(default = "default_stt_binary_path")]
    pub stt_binary_path: String,

    /// Path to the Bark TTS Python wrapper script.
    #[serde(default = "default_bark_binary_path")]
    pub bark_binary_path: String,
}

fn default_tts_voices_dir() -> String {
    "assets/voices".to_string()
}

fn default_tts_binary_path() -> String {
    "assets/piper/piper".to_string()
}

fn default_stt_model_path() -> String {
    "assets/models/ggml-base.en.bin".to_string()
}

fn default_stt_binary_path() -> String {
    "assets/whisper/whisper".to_string()
}

fn default_bark_binary_path() -> String {
    "assets/bark/bark_tts.py".to_string()
}

impl Default for VoicePathsConfig {
    fn default() -> Self {
        Self {
            tts_voices_dir: default_tts_voices_dir(),
            tts_binary_path: default_tts_binary_path(),
            stt_model_path: default_stt_model_path(),
            stt_binary_path: default_stt_binary_path(),
            bark_binary_path: default_bark_binary_path(),
        }
    }
}

/// Network configuration for the HTTP server.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Host address to bind to.
    #[serde(default = "default_host")]
    pub host: IpAddr,

    /// Port to listen on.
    #[serde(default = "default_port")]
    pub port: u16,

    /// Interval in seconds for the message retention background task.
    #[serde(default = "default_retention_check_interval_seconds")]
    pub retention_check_interval_seconds: u64,

    /// Inactivity threshold in seconds for graph node pruning.
    #[serde(default = "default_inactivity_threshold_seconds")]
    pub inactivity_threshold_seconds: u64,

    /// Public URL of the server (e.g. "https://annex.example.com").
    #[serde(default = "default_public_url")]
    pub public_url: String,

    /// Depth of the Merkle tree for identity commitments.
    /// Capacity = 2^depth leaves. Default: 20 (1,048,576 identities).
    #[serde(default = "default_merkle_tree_depth")]
    pub merkle_tree_depth: usize,

    /// Capacity of the tokio broadcast channel for presence SSE events.
    /// Default: 256.
    #[serde(default = "default_presence_broadcast_capacity")]
    pub presence_broadcast_capacity: usize,
}

/// Database configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file.
    #[serde(default = "default_db_path")]
    pub path: String,

    /// Busy timeout for SQLite connections, in milliseconds.
    #[serde(default = "default_db_busy_timeout_ms")]
    pub busy_timeout_ms: u64,

    /// Maximum number of pooled SQLite connections.
    #[serde(default = "default_db_pool_max_size")]
    pub pool_max_size: u32,
}

/// Logging configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    /// Log level filter (e.g., "info", "debug", "annex_server=debug,info").
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Whether to output logs in JSON format.
    #[serde(default)]
    pub json: bool,
}

fn default_host() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
}

fn default_port() -> u16 {
    3000
}

fn default_retention_check_interval_seconds() -> u64 {
    3600
}

fn default_inactivity_threshold_seconds() -> u64 {
    300
}

fn default_public_url() -> String {
    String::new()
}

fn default_db_path() -> String {
    "annex.db".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_merkle_tree_depth() -> usize {
    20
}

fn default_presence_broadcast_capacity() -> usize {
    256
}

fn default_db_busy_timeout_ms() -> u64 {
    5_000
}

fn default_db_pool_max_size() -> u32 {
    8
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            retention_check_interval_seconds: default_retention_check_interval_seconds(),
            inactivity_threshold_seconds: default_inactivity_threshold_seconds(),
            public_url: default_public_url(),
            merkle_tree_depth: default_merkle_tree_depth(),
            presence_broadcast_capacity: default_presence_broadcast_capacity(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
            busy_timeout_ms: default_db_busy_timeout_ms(),
            pool_max_size: default_db_pool_max_size(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            json: false,
        }
    }
}

/// Errors that can occur when loading configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read the configuration file.
    #[error("failed to read config file: {0}")]
    FileRead(#[from] std::io::Error),

    /// Failed to parse the configuration file.
    #[error("failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),

    /// Environment variable value was invalid for the expected type.
    #[error("invalid environment variable {name}: {reason}")]
    InvalidEnvVar { name: &'static str, reason: String },

    /// Configuration value is outside the allowed range.
    #[error("invalid configuration value for {field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },
}

const MIN_DB_BUSY_TIMEOUT_MS: u64 = 1;
const MAX_DB_BUSY_TIMEOUT_MS: u64 = 60_000;
const MIN_DB_POOL_MAX_SIZE: u32 = 1;
const MAX_DB_POOL_MAX_SIZE: u32 = 64;
const MIN_RETENTION_CHECK_INTERVAL_SECONDS: u64 = 1;

fn validate_config(config: &Config) -> Result<(), ConfigError> {
    if !(MIN_DB_BUSY_TIMEOUT_MS..=MAX_DB_BUSY_TIMEOUT_MS).contains(&config.database.busy_timeout_ms)
    {
        return Err(ConfigError::InvalidValue {
            field: "database.busy_timeout_ms",
            reason: format!(
                "must be in range {MIN_DB_BUSY_TIMEOUT_MS}..={MAX_DB_BUSY_TIMEOUT_MS}, got {}",
                config.database.busy_timeout_ms
            ),
        });
    }

    if !(MIN_DB_POOL_MAX_SIZE..=MAX_DB_POOL_MAX_SIZE).contains(&config.database.pool_max_size) {
        return Err(ConfigError::InvalidValue {
            field: "database.pool_max_size",
            reason: format!(
                "must be in range {MIN_DB_POOL_MAX_SIZE}..={MAX_DB_POOL_MAX_SIZE}, got {}",
                config.database.pool_max_size
            ),
        });
    }

    if config.server.retention_check_interval_seconds < MIN_RETENTION_CHECK_INTERVAL_SECONDS {
        return Err(ConfigError::InvalidValue {
            field: "server.retention_check_interval_seconds",
            reason: format!(
                "must be >= {MIN_RETENTION_CHECK_INTERVAL_SECONDS}, got {}",
                config.server.retention_check_interval_seconds
            ),
        });
    }

    if !(1..=30).contains(&config.server.merkle_tree_depth) {
        return Err(ConfigError::InvalidValue {
            field: "server.merkle_tree_depth",
            reason: format!(
                "must be in range 1..=30, got {}",
                config.server.merkle_tree_depth
            ),
        });
    }

    if !(16..=10_000).contains(&config.server.presence_broadcast_capacity) {
        return Err(ConfigError::InvalidValue {
            field: "server.presence_broadcast_capacity",
            reason: format!(
                "must be in range 16..=10000, got {}",
                config.server.presence_broadcast_capacity
            ),
        });
    }

    Ok(())
}

fn parse_env_var<T>(name: &'static str) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(raw) => {
            let parsed = raw.parse::<T>().map_err(|err| ConfigError::InvalidEnvVar {
                name,
                reason: err.to_string(),
            })?;
            Ok(Some(parsed))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidEnvVar {
            name,
            reason: "value is not valid unicode".to_string(),
        }),
    }
}

fn parse_env_bool(name: &'static str) -> Result<Option<bool>, ConfigError> {
    match std::env::var(name) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            let parsed = match normalized.as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => None,
            }
            .ok_or_else(|| ConfigError::InvalidEnvVar {
                name,
                reason: format!("expected one of [true,false,1,0,yes,no,on,off], got '{raw}'"),
            })?;
            Ok(Some(parsed))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidEnvVar {
            name,
            reason: "value is not valid unicode".to_string(),
        }),
    }
}

/// Loads configuration from a TOML file, falling back to defaults.
///
/// Environment variable overrides:
/// - `ANNEX_HOST` overrides `server.host`
/// - `ANNEX_PORT` overrides `server.port`
/// - `ANNEX_DB_PATH` overrides `database.path`
/// - `ANNEX_DB_BUSY_TIMEOUT_MS` overrides `database.busy_timeout_ms`
/// - `ANNEX_DB_POOL_MAX_SIZE` overrides `database.pool_max_size`
/// - `ANNEX_LOG_LEVEL` overrides `logging.level`
/// - `ANNEX_LOG_JSON` overrides `logging.json` (set to "true" to enable)
/// - `ANNEX_TTS_VOICES_DIR` overrides `voice.tts_voices_dir`
/// - `ANNEX_TTS_BINARY_PATH` overrides `voice.tts_binary_path`
/// - `ANNEX_STT_MODEL_PATH` overrides `voice.stt_model_path`
/// - `ANNEX_STT_BINARY_PATH` overrides `voice.stt_binary_path`
///
/// # Errors
///
/// Returns `ConfigError` if the file exists but cannot be read or parsed.
pub fn load_config(path: Option<&str>) -> Result<Config, ConfigError> {
    let mut config = match path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(contents) => toml::from_str(&contents)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = p, "config file not found, using defaults");
                Config::default()
            }
            Err(e) => return Err(ConfigError::FileRead(e)),
        },
        None => Config::default(),
    };

    // Environment variable overrides
    if let Some(host) = parse_env_var("ANNEX_HOST")? {
        config.server.host = host;
    }
    if let Some(port) = parse_env_var("ANNEX_PORT")? {
        config.server.port = port;
    }
    if let Some(interval) = parse_env_var("ANNEX_RETENTION_CHECK_INTERVAL_SECONDS")? {
        config.server.retention_check_interval_seconds = interval;
    }
    if let Some(threshold) = parse_env_var("ANNEX_INACTIVITY_THRESHOLD_SECONDS")? {
        config.server.inactivity_threshold_seconds = threshold;
    }
    if let Some(public_url) = parse_env_var("ANNEX_PUBLIC_URL")? {
        config.server.public_url = public_url;
    }
    if let Some(depth) = parse_env_var("ANNEX_MERKLE_TREE_DEPTH")? {
        config.server.merkle_tree_depth = depth;
    }
    if let Some(cap) = parse_env_var("ANNEX_PRESENCE_BROADCAST_CAPACITY")? {
        config.server.presence_broadcast_capacity = cap;
    }
    if let Some(db_path) = parse_env_var::<String>("ANNEX_DB_PATH")? {
        config.database.path = db_path;
    }
    if let Some(timeout) = parse_env_var("ANNEX_DB_BUSY_TIMEOUT_MS")? {
        config.database.busy_timeout_ms = timeout;
    }
    if let Some(max_size) = parse_env_var("ANNEX_DB_POOL_MAX_SIZE")? {
        config.database.pool_max_size = max_size;
    }
    if let Some(level) = parse_env_var::<String>("ANNEX_LOG_LEVEL")? {
        config.logging.level = level;
    }
    if let Some(json) = parse_env_bool("ANNEX_LOG_JSON")? {
        config.logging.json = json;
    }
    if let Some(url) = parse_env_var("ANNEX_LIVEKIT_URL")? {
        config.livekit.url = url;
    }
    if let Some(public_url) = parse_env_var::<String>("ANNEX_LIVEKIT_PUBLIC_URL")? {
        config.livekit.public_url = public_url;
    }
    if let Some(api_key) = parse_env_var("ANNEX_LIVEKIT_API_KEY")? {
        config.livekit.api_key = api_key;
    }
    if let Some(api_secret) = parse_env_var("ANNEX_LIVEKIT_API_SECRET")? {
        config.livekit.api_secret = api_secret;
    }
    if let Some(val) = parse_env_var::<String>("ANNEX_TTS_VOICES_DIR")? {
        config.voice.tts_voices_dir = val;
    }
    if let Some(val) = parse_env_var::<String>("ANNEX_TTS_BINARY_PATH")? {
        config.voice.tts_binary_path = val;
    }
    if let Some(val) = parse_env_var::<String>("ANNEX_STT_MODEL_PATH")? {
        config.voice.stt_model_path = val;
    }
    if let Some(val) = parse_env_var::<String>("ANNEX_STT_BINARY_PATH")? {
        config.voice.stt_binary_path = val;
    }
    if let Some(val) = parse_env_var::<String>("ANNEX_BARK_BINARY_PATH")? {
        config.voice.bark_binary_path = val;
    }
    if let Ok(origins) = std::env::var("ANNEX_CORS_ORIGINS") {
        config.cors.allowed_origins = origins
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Some(enforce) = parse_env_bool("ANNEX_ENFORCE_ZK_PROOFS")? {
        config.security.enforce_zk_proofs = enforce;
    }

    validate_config(&config)?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn clear_env() {
        std::env::remove_var("ANNEX_HOST");
        std::env::remove_var("ANNEX_PORT");
        std::env::remove_var("ANNEX_DB_PATH");
        std::env::remove_var("ANNEX_DB_BUSY_TIMEOUT_MS");
        std::env::remove_var("ANNEX_DB_POOL_MAX_SIZE");
        std::env::remove_var("ANNEX_LOG_LEVEL");
        std::env::remove_var("ANNEX_LOG_JSON");
        std::env::remove_var("ANNEX_TTS_VOICES_DIR");
        std::env::remove_var("ANNEX_TTS_BINARY_PATH");
        std::env::remove_var("ANNEX_STT_MODEL_PATH");
        std::env::remove_var("ANNEX_STT_BINARY_PATH");
    }

    fn write_temp_config(contents: &str) -> String {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let file_name = format!("annex-config-{unique_suffix}.toml");
        let path = std::env::temp_dir().join(file_name);
        fs::write(&path, contents).expect("failed to write temp config");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn defaults_are_loaded_when_file_missing() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let cfg = load_config(Some("this-file-does-not-exist.toml")).expect("load should succeed");

        assert_eq!(cfg.server.host, default_host());
        assert_eq!(cfg.server.port, default_port());
        assert_eq!(cfg.database.path, default_db_path());
        assert_eq!(cfg.database.busy_timeout_ms, default_db_busy_timeout_ms());
        assert_eq!(cfg.database.pool_max_size, default_db_pool_max_size());
        assert_eq!(cfg.logging.level, default_log_level());
        assert!(!cfg.logging.json);
        assert_eq!(cfg.voice.tts_voices_dir, default_tts_voices_dir());
        assert_eq!(cfg.voice.tts_binary_path, default_tts_binary_path());
        assert_eq!(cfg.voice.stt_model_path, default_stt_model_path());
        assert_eq!(cfg.voice.stt_binary_path, default_stt_binary_path());
    }

    #[test]
    fn explicit_config_path_is_loaded() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[server]
host = "0.0.0.0"
port = 4567

[database]
path = "path-from-file.db"
busy_timeout_ms = 15000
pool_max_size = 32

[logging]
level = "trace"
json = true
"#,
        );

        let cfg = load_config(Some(path.as_str())).expect("load should succeed");

        assert_eq!(cfg.server.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(cfg.server.port, 4567);
        assert_eq!(cfg.database.path, "path-from-file.db");
        assert_eq!(cfg.database.busy_timeout_ms, 15_000);
        assert_eq!(cfg.database.pool_max_size, 32);
        assert_eq!(cfg.logging.level, "trace");
        assert!(cfg.logging.json);

        fs::remove_file(path).expect("failed to remove temp config");
    }

    #[test]
    fn env_overrides_are_applied() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_HOST", "0.0.0.0");
        std::env::set_var("ANNEX_PORT", "9876");
        std::env::set_var("ANNEX_DB_PATH", "custom.db");
        std::env::set_var("ANNEX_DB_BUSY_TIMEOUT_MS", "12000");
        std::env::set_var("ANNEX_DB_POOL_MAX_SIZE", "16");
        std::env::set_var("ANNEX_LOG_LEVEL", "debug");
        std::env::set_var("ANNEX_LOG_JSON", "yes");

        let cfg = load_config(None).expect("load should succeed");

        assert_eq!(cfg.server.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(cfg.server.port, 9876);
        assert_eq!(cfg.database.path, "custom.db");
        assert_eq!(cfg.database.busy_timeout_ms, 12_000);
        assert_eq!(cfg.database.pool_max_size, 16);
        assert_eq!(cfg.logging.level, "debug");
        assert!(cfg.logging.json);

        clear_env();
    }

    #[test]
    fn invalid_port_env_returns_error() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_PORT", "invalid-port");

        let err = load_config(None).expect_err("load should fail for invalid port");
        match err {
            ConfigError::InvalidEnvVar { name, .. } => assert_eq!(name, "ANNEX_PORT"),
            other => panic!("unexpected error: {other}"),
        }

        clear_env();
    }

    #[test]
    fn invalid_json_bool_env_returns_error() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_LOG_JSON", "definitely");

        let err = load_config(None).expect_err("load should fail for invalid bool value");
        match err {
            ConfigError::InvalidEnvVar { name, .. } => assert_eq!(name, "ANNEX_LOG_JSON"),
            other => panic!("unexpected error: {other}"),
        }

        clear_env();
    }

    #[test]
    fn out_of_range_busy_timeout_returns_error() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_DB_BUSY_TIMEOUT_MS", "0");

        let err = load_config(None).expect_err("load should fail for out-of-range timeout");
        match err {
            ConfigError::InvalidValue { field, .. } => {
                assert_eq!(field, "database.busy_timeout_ms")
            }
            other => panic!("unexpected error: {other}"),
        }

        clear_env();
    }

    #[test]
    fn out_of_range_pool_max_size_returns_error() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_DB_POOL_MAX_SIZE", "0");

        let err = load_config(None).expect_err("load should fail for out-of-range pool size");
        match err {
            ConfigError::InvalidValue { field, .. } => {
                assert_eq!(field, "database.pool_max_size")
            }
            other => panic!("unexpected error: {other}"),
        }

        clear_env();
    }

    #[test]
    fn voice_paths_env_overrides() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_TTS_VOICES_DIR", "/opt/voices");
        std::env::set_var("ANNEX_TTS_BINARY_PATH", "/usr/bin/piper");
        std::env::set_var("ANNEX_STT_MODEL_PATH", "/opt/models/whisper.bin");
        std::env::set_var("ANNEX_STT_BINARY_PATH", "/usr/bin/whisper");

        let cfg = load_config(None).expect("load should succeed");

        assert_eq!(cfg.voice.tts_voices_dir, "/opt/voices");
        assert_eq!(cfg.voice.tts_binary_path, "/usr/bin/piper");
        assert_eq!(cfg.voice.stt_model_path, "/opt/models/whisper.bin");
        assert_eq!(cfg.voice.stt_binary_path, "/usr/bin/whisper");

        clear_env();
    }

    #[test]
    fn voice_paths_from_config_file() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[voice]
tts_voices_dir = "/from/config/voices"
tts_binary_path = "/from/config/piper"
stt_model_path = "/from/config/ggml.bin"
stt_binary_path = "/from/config/whisper"
"#,
        );

        let cfg = load_config(Some(path.as_str())).expect("load should succeed");

        assert_eq!(cfg.voice.tts_voices_dir, "/from/config/voices");
        assert_eq!(cfg.voice.tts_binary_path, "/from/config/piper");
        assert_eq!(cfg.voice.stt_model_path, "/from/config/ggml.bin");
        assert_eq!(cfg.voice.stt_binary_path, "/from/config/whisper");

        fs::remove_file(path).expect("failed to remove temp config");
    }

    #[test]
    fn voice_paths_env_overrides_config_file() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[voice]
tts_voices_dir = "/from/config/voices"
"#,
        );

        std::env::set_var("ANNEX_TTS_VOICES_DIR", "/from/env/voices");

        let cfg = load_config(Some(path.as_str())).expect("load should succeed");

        // Env should override config file
        assert_eq!(cfg.voice.tts_voices_dir, "/from/env/voices");
        // Other fields should remain at defaults
        assert_eq!(cfg.voice.tts_binary_path, default_tts_binary_path());

        fs::remove_file(path).expect("failed to remove temp config");
        clear_env();
    }

    #[test]
    fn zero_retention_check_interval_returns_error() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[server]
retention_check_interval_seconds = 0
"#,
        );

        let err = load_config(Some(path.as_str()))
            .expect_err("load should fail for zero retention interval");
        match err {
            ConfigError::InvalidValue { field, .. } => {
                assert_eq!(field, "server.retention_check_interval_seconds")
            }
            other => panic!("unexpected error: {other}"),
        }

        fs::remove_file(path).expect("failed to remove temp config");
        clear_env();
    }
}
