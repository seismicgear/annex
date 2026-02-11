//! Server configuration loading from file and environment variables.

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
}

/// Database configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file.
    #[serde(default = "default_db_path")]
    pub path: String,
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

fn default_db_path() -> String {
    "annex.db".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
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
/// - `ANNEX_LOG_LEVEL` overrides `logging.level`
/// - `ANNEX_LOG_JSON` overrides `logging.json` (set to "true" to enable)
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
    if let Ok(db_path) = std::env::var("ANNEX_DB_PATH") {
        config.database.path = db_path;
    }
    if let Ok(level) = std::env::var("ANNEX_LOG_LEVEL") {
        config.logging.level = level;
    }
    if let Some(json) = parse_env_bool("ANNEX_LOG_JSON")? {
        config.logging.json = json;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn clear_env() {
        std::env::remove_var("ANNEX_HOST");
        std::env::remove_var("ANNEX_PORT");
        std::env::remove_var("ANNEX_DB_PATH");
        std::env::remove_var("ANNEX_LOG_LEVEL");
        std::env::remove_var("ANNEX_LOG_JSON");
    }

    #[test]
    fn defaults_are_loaded_when_file_missing() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let cfg = load_config(Some("this-file-does-not-exist.toml")).expect("load should succeed");

        assert_eq!(cfg.server.host, default_host());
        assert_eq!(cfg.server.port, default_port());
        assert_eq!(cfg.database.path, default_db_path());
        assert_eq!(cfg.logging.level, default_log_level());
        assert!(!cfg.logging.json);
    }

    #[test]
    fn env_overrides_are_applied() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        std::env::set_var("ANNEX_HOST", "0.0.0.0");
        std::env::set_var("ANNEX_PORT", "9876");
        std::env::set_var("ANNEX_DB_PATH", "custom.db");
        std::env::set_var("ANNEX_LOG_LEVEL", "debug");
        std::env::set_var("ANNEX_LOG_JSON", "yes");

        let cfg = load_config(None).expect("load should succeed");

        assert_eq!(cfg.server.host, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(cfg.server.port, 9876);
        assert_eq!(cfg.database.path, "custom.db");
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
}
