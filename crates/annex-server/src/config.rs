//! Server configuration loading from file and environment variables.

use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr};
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
    if let Ok(host) = std::env::var("ANNEX_HOST") {
        if let Ok(parsed) = host.parse() {
            config.server.host = parsed;
        }
    }
    if let Ok(port) = std::env::var("ANNEX_PORT") {
        if let Ok(parsed) = port.parse() {
            config.server.port = parsed;
        }
    }
    if let Ok(db_path) = std::env::var("ANNEX_DB_PATH") {
        config.database.path = db_path;
    }
    if let Ok(level) = std::env::var("ANNEX_LOG_LEVEL") {
        config.logging.level = level;
    }
    if let Ok(json) = std::env::var("ANNEX_LOG_JSON") {
        config.logging.json = json == "true" || json == "1";
    }

    Ok(config)
}
