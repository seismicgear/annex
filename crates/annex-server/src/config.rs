//! Server configuration loading from file and environment variables.

use annex_voice::WebRtcConfig;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;

/// Top-level server configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

    /// Native WebRTC configuration (signaling/STUN/TURN).
    #[serde(default)]
    pub webrtc: WebRtcConfig,

    /// Voice pipeline paths (TTS binary, STT model, etc.).
    #[serde(default)]
    pub voice: VoicePathsConfig,

    /// CORS configuration.
    #[serde(default)]
    pub cors: CorsConfig,

    /// Security enforcement settings.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Federation reliability (freshness gate, outbox retry policy).
    #[serde(default)]
    pub federation: FederationConfig,

    /// Storage health thresholds + SQLite maintenance schedule.
    #[serde(default)]
    pub storage: StorageConfig,

    /// Deployment topology + rate-limit backend.
    #[serde(default)]
    pub deployment: DeploymentConfig,
}

/// Federation reliability knobs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FederationConfig {
    /// Maximum age (seconds) of a *live* federated envelope's
    /// `created_at` before the receiver rejects it. Envelopes
    /// delivered through `/api/federation/catch-up` are NOT bound by
    /// this window — see `crates/annex-server/src/services/federation_service.rs`.
    #[serde(default = "default_freshness_window_seconds")]
    pub freshness_window_seconds: i64,

    /// Maximum future skew (seconds) allowed on a federated envelope's
    /// `created_at`. Anything more than this far in the future is
    /// rejected as clock-skewed or deliberately forward-dated.
    #[serde(default = "default_future_skew_seconds")]
    pub future_skew_seconds: i64,

    /// Outbox worker tick interval (seconds).
    #[serde(default = "default_outbox_interval_seconds")]
    pub outbox_interval_seconds: u64,

    /// Max delivery attempts before an outbox row is marked `failed`.
    /// Defaults to 12 → with the bounded-exponential backoff schedule
    /// this is ~3 hours of retries before giving up.
    #[serde(default = "default_outbox_max_attempts")]
    pub outbox_max_attempts: u32,

    /// Max outbox rows drained per peer per worker tick. Caps how much
    /// of the batch a single (possibly misbehaving) peer can occupy, so
    /// one unreachable peer with a deep backlog cannot starve delivery
    /// to healthy peers or burn the whole tick's HTTP budget against
    /// itself. Values below 1 are treated as 1. Defaults to 8.
    #[serde(default = "default_outbox_per_peer_batch")]
    pub outbox_per_peer_batch: u32,

    /// Default envelope version produced on the outbound side. Stays at
    /// `"v1"` for one release so peers can pick up the v2 verifier
    /// before the sender flips to v2.
    #[serde(default = "default_outbound_envelope_version")]
    pub default_outbound_envelope_version: String,
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            freshness_window_seconds: default_freshness_window_seconds(),
            future_skew_seconds: default_future_skew_seconds(),
            outbox_interval_seconds: default_outbox_interval_seconds(),
            outbox_max_attempts: default_outbox_max_attempts(),
            outbox_per_peer_batch: default_outbox_per_peer_batch(),
            default_outbound_envelope_version: default_outbound_envelope_version(),
        }
    }
}

fn default_freshness_window_seconds() -> i64 {
    300
}
fn default_future_skew_seconds() -> i64 {
    60
}
fn default_outbox_interval_seconds() -> u64 {
    5
}
fn default_outbox_max_attempts() -> u32 {
    12
}
fn default_outbox_per_peer_batch() -> u32 {
    8
}
fn default_outbound_envelope_version() -> String {
    annex_federation::FEDERATED_MESSAGE_ENVELOPE_V1.to_string()
}

/// Storage health + SQLite maintenance.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    /// Free disk bytes at which the server logs a warning. Reads still
    /// flow; writes still flow. Operational signal only.
    #[serde(default = "default_storage_warn_free_bytes")]
    pub warn_free_bytes: u64,

    /// Free disk bytes at which the server refuses writes with HTTP
    /// 507 / a WS storage-error frame. Reads continue to flow. The
    /// retention sweep and maintenance VACUUM are still allowed to run
    /// because they can reduce storage pressure.
    #[serde(default = "default_storage_block_free_bytes")]
    pub block_free_bytes: u64,

    /// Enable periodic SQLite maintenance. Runs
    /// `PRAGMA wal_checkpoint(TRUNCATE)`, `ANALYZE`, and optionally
    /// `VACUUM`. Off by default — operators opt in via env.
    #[serde(default)]
    pub maintenance_enabled: bool,

    /// Hours between maintenance sweeps.
    #[serde(default = "default_maintenance_interval_hours")]
    pub maintenance_interval_hours: u64,

    /// Whether to run `VACUUM` (full-table rewrite, blocks writers)
    /// during the maintenance window. Off by default; on means the
    /// operator has accepted the blocking-write tradeoff during the
    /// chosen window.
    #[serde(default)]
    pub maintenance_vacuum: bool,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            warn_free_bytes: default_storage_warn_free_bytes(),
            block_free_bytes: default_storage_block_free_bytes(),
            maintenance_enabled: false,
            maintenance_interval_hours: default_maintenance_interval_hours(),
            maintenance_vacuum: false,
        }
    }
}

fn default_storage_warn_free_bytes() -> u64 {
    536_870_912 // 512 MiB
}
fn default_storage_block_free_bytes() -> u64 {
    67_108_864 // 64 MiB
}
fn default_maintenance_interval_hours() -> u64 {
    24
}

/// Security enforcement configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityConfig {
    /// When true, channel access endpoints require a valid ZK membership proof
    /// via the `x-annex-zk-proof` header. Default: true.
    ///
    /// Set to false only during development or when ZK trusted setup has not
    /// been completed. Running with this disabled undermines identity plane
    /// guarantees.
    #[serde(default = "default_enforce_zk_proofs")]
    pub enforce_zk_proofs: bool,

    /// Membership-circuit versions accepted by this server. Default: `["v1"]`.
    ///
    /// Each enabled version requires its corresponding verification key to
    /// be loadable at startup (when `enforce_zk_proofs` is true). The server
    /// dispatches incoming proof payloads to the matching verifier by an
    /// explicit `protocol_version` field — it does NOT silently mix v1 and
    /// v2 semantics.
    ///
    /// Recognised values: `"v1"` (commitment-derived nullifier; legacy) and
    /// `"v2"` (secret-derived nullifier; production target). Unknown values
    /// surface as a startup error.
    #[serde(default = "default_enabled_zk_versions")]
    pub enabled_zk_versions: Vec<String>,
}

fn default_enforce_zk_proofs() -> bool {
    true
}

fn default_enabled_zk_versions() -> Vec<String> {
    vec!["v1".to_string()]
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enforce_zk_proofs: default_enforce_zk_proofs(),
            enabled_zk_versions: default_enabled_zk_versions(),
        }
    }
}

/// CORS (Cross-Origin Resource Sharing) configuration.
///
/// By default, CORS is **restrictive** (same-origin only). To allow cross-origin
/// requests, set `allowed_origins` to a list of origin URLs or `["*"]` for
/// permissive mode.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CorsConfig {
    /// List of allowed origins. Empty = same-origin only. `["*"]` = allow all.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

/// Deployment topology — what the operator promises about the network
/// shape around this process, plus the backend used for rate limiting.
///
/// The fields here are read by the rate-limit middleware to decide which
/// client IP to key on, and by config validation to refuse impossible
/// production setups (e.g. clustered deployment with an in-memory rate
/// limiter that would silently let each replica grant the full per-IP
/// budget).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeploymentConfig {
    /// Number of trusted reverse-proxy hops in front of this server.
    ///
    /// * `0` (default) — direct exposure; the `ConnectInfo` socket address
    ///   is the client. `X-Forwarded-For` is NEVER trusted for rate-limit
    ///   keying; a malicious client cannot evade per-IP limits by adding
    ///   that header.
    /// * `N >= 1` — the operator has N proxies in front. Each proxy
    ///   appends the immediate-source IP, so the real client is the
    ///   left-most entry beyond the `N` trusted hops. The middleware
    ///   takes `X-Forwarded-For[len - N - 1]` (clamped to the leftmost
    ///   entry when the list is shorter). Misconfiguring N higher than
    ///   the actual hop count exposes the rate limiter to spoofing; this
    ///   is an operator decision, never auto-detected.
    #[serde(default = "default_trusted_proxy_depth")]
    pub trusted_proxy_depth: u8,

    /// Deployment mode: `"single"` (default) or `"clustered"`.
    ///
    /// Under `clustered`, the rate-limit backend MUST be a shared store
    /// — an in-memory backend would only enforce its budget on the
    /// replica that handled the request, multiplying the effective
    /// limit by the replica count. Production refuses to start in this
    /// mismatched configuration.
    #[serde(default = "default_deployment_mode")]
    pub mode: String,

    /// Rate-limit backend identifier. `"memory"` (default) uses the
    /// in-process `RateLimiter` in `middleware.rs`. Any other value
    /// indicates an externally-provisioned shared store (e.g.
    /// `"redis"`); the server doesn't ship a remote backend itself
    /// yet — operators wire one through a future
    /// `RateLimitBackend` trait. Setting a non-`memory` value while
    /// the trait is unimplemented surfaces as a startup error rather
    /// than a silent fallback.
    #[serde(default = "default_rate_limit_backend")]
    pub rate_limit_backend: String,

    /// Opt-in to the experimental relay-based federation transport
    /// (`crates/annex-federation/src/transport.rs`). Defaults to
    /// `false` and SHOULD stay that way in production until the
    /// `signal_verifier` callback has a real slug↔pubkey binding
    /// implementation (see the module banner there). Setting this to
    /// `true` under a production profile is rejected at startup
    /// unless `ANNEX_SIGNAL_TRUSTED_PEERS` is also configured — a
    /// belt-and-suspenders check that catches half-wired operator
    /// configs before the server accepts any federated SDP.
    #[serde(default = "default_relay_transport_enabled")]
    pub experimental_relay_transport_enabled: bool,
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            trusted_proxy_depth: default_trusted_proxy_depth(),
            mode: default_deployment_mode(),
            rate_limit_backend: default_rate_limit_backend(),
            experimental_relay_transport_enabled: default_relay_transport_enabled(),
        }
    }
}

fn default_trusted_proxy_depth() -> u8 {
    0
}
fn default_deployment_mode() -> String {
    "single".to_string()
}
fn default_rate_limit_backend() -> String {
    "memory".to_string()
}
fn default_relay_transport_enabled() -> bool {
    false
}

/// File-system paths for the TTS and STT voice pipelines.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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

    /// Age in seconds past which WS-idempotency ledger rows
    /// (`message_request_ids`, ADR-0010) are evicted by the retention
    /// task. After eviction a replayed `clientRequestId` is treated as
    /// a new send, so this must comfortably exceed any client retry
    /// window. Default: 604800 (7 days).
    #[serde(default = "default_idempotency_ttl_seconds")]
    pub idempotency_ttl_seconds: u64,

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

    /// Base URL for generated invite links.
    /// Default: "https://monolithannex.com/invite".
    #[serde(default = "default_invite_base_url")]
    pub invite_base_url: String,

    /// Stable, random slug used as this node's sovereign federation identity.
    /// Auto-generated on first boot when not present in config.toml.
    #[serde(default)]
    pub server_slug: String,
}

/// Database configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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

fn default_idempotency_ttl_seconds() -> u64 {
    // 7 days. The ledger only needs to outlive the client retry
    // horizon (seconds to minutes); a week of slack keeps replay
    // protection across long client outages while bounding table
    // growth on busy servers.
    604_800
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

fn default_invite_base_url() -> String {
    "https://monolithannex.com/invite".to_string()
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
            idempotency_ttl_seconds: default_idempotency_ttl_seconds(),
            inactivity_threshold_seconds: default_inactivity_threshold_seconds(),
            public_url: default_public_url(),
            merkle_tree_depth: default_merkle_tree_depth(),
            presence_broadcast_capacity: default_presence_broadcast_capacity(),
            invite_base_url: default_invite_base_url(),
            server_slug: String::new(),
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
/// A tiny idempotency TTL would evict ledger rows while a client is
/// still inside its retry window, silently re-opening the replay
/// surface ADR-0010 closed. One minute is the floor; the default is
/// 7 days.
const MIN_IDEMPOTENCY_TTL_SECONDS: u64 = 60;

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

    if config.server.idempotency_ttl_seconds < MIN_IDEMPOTENCY_TTL_SECONDS {
        return Err(ConfigError::InvalidValue {
            field: "server.idempotency_ttl_seconds",
            reason: format!(
                "must be >= {MIN_IDEMPOTENCY_TTL_SECONDS}, got {}",
                config.server.idempotency_ttl_seconds
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

    validate_cors_for_build_profile(&config.cors)?;
    validate_deployment_for_build_profile(&config.deployment)?;

    Ok(())
}

/// Refuse impossible deployment shapes under a production profile.
///
/// Two gates:
///
/// * Mode `"clustered"` MUST pair with a non-memory rate-limit backend.
///   An in-memory limiter on a clustered deployment gives each replica
///   its own bucket — the effective limit becomes `backend_limit * N`,
///   silently letting an authenticated user (or one IP) do N× the
///   intended traffic. Operators who want shared rate limiting must
///   wire an external backend; until then, clustered production is
///   rejected at startup rather than silently broken.
///
/// * `trusted_proxy_depth` is capped at 16. Each hop trusts the next-
///   upstream's `X-Forwarded-For` blindly, so a misconfigured-high
///   depth is a footgun: it would believe an arbitrary leftmost entry
///   the client itself wrote. 16 hops is far above any real CDN+LB
///   stack and stops typos from `8` becoming `888`.
fn validate_deployment_for_build_profile(deployment: &DeploymentConfig) -> Result<(), ConfigError> {
    let mode = deployment.mode.trim().to_ascii_lowercase();
    if mode != "single" && mode != "clustered" {
        return Err(ConfigError::InvalidValue {
            field: "deployment.mode",
            reason: format!(
                "must be \"single\" or \"clustered\", got {:?}",
                deployment.mode
            ),
        });
    }

    if deployment.trusted_proxy_depth > 16 {
        return Err(ConfigError::InvalidValue {
            field: "deployment.trusted_proxy_depth",
            reason: format!(
                "must be 0..=16, got {}. A depth above any plausible \
                 reverse-proxy chain is almost always a typo and lets a \
                 spoofed X-Forwarded-For evade the rate limiter.",
                deployment.trusted_proxy_depth
            ),
        });
    }

    let raw_profile = match std::env::var("ANNEX_BUILD_PROFILE") {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let profile = raw_profile.trim().to_ascii_lowercase();
    if profile != "production" && profile != "release" {
        return Ok(());
    }

    if mode == "clustered"
        && deployment
            .rate_limit_backend
            .trim()
            .eq_ignore_ascii_case("memory")
    {
        return Err(ConfigError::InvalidValue {
            field: "deployment.rate_limit_backend",
            reason: format!(
                "ANNEX_DEPLOYMENT_MODE=clustered under ANNEX_BUILD_PROFILE={raw_profile} \
                 requires a shared rate-limit backend. The in-memory backend gives each \
                 replica its own bucket, multiplying the effective limit by the replica \
                 count. Set ANNEX_RATE_LIMIT_BACKEND to a shared store, or run single-mode."
            ),
        });
    }

    // No remote backends are implemented yet; anything other than
    // `memory` would silently no-op. Surface that mismatch loudly
    // instead of pretending we enforced a remote limit.
    let backend = deployment.rate_limit_backend.trim().to_ascii_lowercase();
    if backend != "memory" {
        return Err(ConfigError::InvalidValue {
            field: "deployment.rate_limit_backend",
            reason: format!(
                "rate_limit_backend={backend:?} requires a `RateLimitBackend` \
                 implementation that this build does not ship. Either implement the \
                 adapter and remove this gate, or use \"memory\" under single-mode."
            ),
        });
    }

    // Experimental relay transport. Flipping this to true under a
    // production profile is allowed only if the operator has ALSO
    // configured the relay's trust map (`ANNEX_SIGNAL_TRUSTED_PEERS`).
    // Without that, the relay would accept signed envelopes from any
    // keypair, defeating the gate.
    if deployment.experimental_relay_transport_enabled {
        let trust = std::env::var("ANNEX_SIGNAL_TRUSTED_PEERS")
            .unwrap_or_default()
            .trim()
            .to_string();
        if trust.is_empty() {
            return Err(ConfigError::InvalidValue {
                field: "deployment.experimental_relay_transport_enabled",
                reason: format!(
                    "ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED=true under ANNEX_BUILD_PROFILE={raw_profile} \
                     requires ANNEX_SIGNAL_TRUSTED_PEERS to be configured. The relay is the only \
                     authorization gate for federation SDP; without it, any holder of an Ed25519 \
                     keypair could inject sessions."
                ),
            });
        }
    }

    Ok(())
}

/// Refuse to start a production build when the resolved CORS policy is
/// wildcard or empty.
///
/// Background: the Docker image used to ship `ANNEX_CORS_ORIGINS=*` baked
/// into the production layer, and there was no runtime guard. Operators
/// who never touched env vars ended up running an internet-facing server
/// that accepted requests from any origin. The Dockerfile no longer sets
/// the wildcard, and this check is the belt to that suspenders: if
/// `ANNEX_BUILD_PROFILE=production` (or `release`) is set, an empty list
/// or any `*` entry is a startup error. Dev profiles (the default) keep
/// their current permissive behaviour so `cargo run -p annex-server` and
/// `docker compose up` still work without per-origin configuration.
///
/// Reads `ANNEX_BUILD_PROFILE` directly because nothing else in the
/// server runtime needs to know the build profile — wiring it into
/// `Config` would force every test fixture to plumb a new field.
fn validate_cors_for_build_profile(cors: &CorsConfig) -> Result<(), ConfigError> {
    let raw_profile = match std::env::var("ANNEX_BUILD_PROFILE") {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let profile = raw_profile.trim().to_ascii_lowercase();
    if profile != "production" && profile != "release" {
        return Ok(());
    }

    let has_wildcard = cors.allowed_origins.iter().any(|o| o.trim() == "*");
    if has_wildcard {
        return Err(ConfigError::InvalidValue {
            field: "cors.allowed_origins",
            reason: format!(
                "wildcard CORS origin (\"*\") is forbidden under ANNEX_BUILD_PROFILE={raw_profile}. \
                 Set ANNEX_CORS_ORIGINS to an explicit comma-separated list of allowed origins \
                 (e.g. https://app.example.com), or run a dev profile."
            ),
        });
    }
    if cors.allowed_origins.is_empty() {
        return Err(ConfigError::InvalidValue {
            field: "cors.allowed_origins",
            reason: format!(
                "no CORS allowed origins configured under ANNEX_BUILD_PROFILE={raw_profile}. \
                 Set ANNEX_CORS_ORIGINS to an explicit comma-separated list (e.g. \
                 https://app.example.com) or cors.allowed_origins in config.toml. \
                 Refusing to start with an unconfigured cross-origin policy under production."
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
            Ok(contents) => {
                // Fix Windows backslash paths before parsing. TOML
                // double-quoted strings treat `\U` as an 8-digit unicode
                // escape, so paths like `C:\Users\...\annex.db` cause parse
                // errors. Replace backslashes with forward slashes when the
                // content contains a drive-letter pattern (e.g. `C:\`).
                let sanitized = if contents.contains(":\\") {
                    let fixed = contents.replace('\\', "/");
                    // Best-effort: persist the fix so the on-disk file is
                    // also valid for manual inspection and future reads.
                    if let Err(e) = atomic_write_file(p, &fixed) {
                        tracing::warn!(
                            path = p,
                            error = %e,
                            "could not persist backslash path fix to config file"
                        );
                    }
                    fixed
                } else {
                    contents
                };
                let mut config: Config = toml::from_str(&sanitized)?;
                let mut wrote_defaults = false;

                if config.server.server_slug.is_empty() {
                    config.server.server_slug =
                        derive_server_slug_from_public_url(&config.server.public_url);
                    wrote_defaults = true;
                }

                if wrote_defaults {
                    if let Err(e) = sync_config_defaults_to_disk(p, &sanitized, &config) {
                        tracing::warn!(
                            path = p,
                            error = %e,
                            "failed to persist autogenerated configuration defaults"
                        );
                    }
                }

                config
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = p, "config file not found, using defaults");
                let mut config = Config::default();
                config.server.server_slug =
                    derive_server_slug_from_public_url(&config.server.public_url);
                if let Err(e) = sync_config_defaults_to_disk(p, "", &config) {
                    tracing::warn!(
                        path = p,
                        error = %e,
                        "failed to write default config file"
                    );
                }
                config
            }
            Err(e) => return Err(ConfigError::FileRead(e)),
        },
        None => {
            let mut config = Config::default();
            config.server.server_slug =
                derive_server_slug_from_public_url(&config.server.public_url);
            config
        }
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
    if let Some(ttl) = parse_env_var("ANNEX_IDEMPOTENCY_TTL_SECONDS")? {
        config.server.idempotency_ttl_seconds = ttl;
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
    if let Some(invite_url) = parse_env_var::<String>("ANNEX_INVITE_BASE_URL")? {
        config.server.invite_base_url = invite_url;
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
    if let Some(url) = parse_env_var("ANNEX_WEBRTC_URL")? {
        config.webrtc.url = url;
    }
    if let Some(public_url) = parse_env_var::<String>("ANNEX_WEBRTC_PUBLIC_URL")? {
        config.webrtc.public_url = public_url;
    }
    if let Some(api_key) = parse_env_var("ANNEX_WEBRTC_API_KEY")? {
        config.webrtc.api_key = api_key;
    }
    if let Some(api_secret) = parse_env_var("ANNEX_WEBRTC_API_SECRET")? {
        config.webrtc.api_secret = api_secret;
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
    if let Some(v) = parse_env_var::<i64>("ANNEX_FEDERATION_FRESHNESS_SECONDS")? {
        config.federation.freshness_window_seconds = v;
    }
    if let Some(v) = parse_env_var::<i64>("ANNEX_FEDERATION_FUTURE_SKEW_SECONDS")? {
        config.federation.future_skew_seconds = v;
    }
    if let Some(v) = parse_env_var::<u64>("ANNEX_FEDERATION_OUTBOX_INTERVAL_SECONDS")? {
        config.federation.outbox_interval_seconds = v;
    }
    if let Some(v) = parse_env_var::<u32>("ANNEX_FEDERATION_OUTBOX_MAX_ATTEMPTS")? {
        config.federation.outbox_max_attempts = v;
    }
    if let Some(v) = parse_env_var::<u32>("ANNEX_FEDERATION_OUTBOX_PER_PEER_BATCH")? {
        config.federation.outbox_per_peer_batch = v;
    }
    if let Some(v) = parse_env_var::<String>("ANNEX_FEDERATION_DEFAULT_ENVELOPE_VERSION")? {
        config.federation.default_outbound_envelope_version = v;
    }
    if let Some(v) = parse_env_var::<u64>("ANNEX_STORAGE_WARN_FREE_BYTES")? {
        config.storage.warn_free_bytes = v;
    }
    if let Some(v) = parse_env_var::<u64>("ANNEX_STORAGE_BLOCK_FREE_BYTES")? {
        config.storage.block_free_bytes = v;
    }
    if let Some(v) = parse_env_bool("ANNEX_DB_MAINTENANCE_ENABLED")? {
        config.storage.maintenance_enabled = v;
    }
    if let Some(v) = parse_env_var::<u64>("ANNEX_DB_MAINTENANCE_INTERVAL_HOURS")? {
        config.storage.maintenance_interval_hours = v;
    }
    if let Some(v) = parse_env_bool("ANNEX_DB_MAINTENANCE_VACUUM")? {
        config.storage.maintenance_vacuum = v;
    }
    if let Some(v) = parse_env_var::<u8>("ANNEX_TRUSTED_PROXY_DEPTH")? {
        config.deployment.trusted_proxy_depth = v;
    }
    if let Some(v) = parse_env_var::<String>("ANNEX_DEPLOYMENT_MODE")? {
        config.deployment.mode = v;
    }
    if let Some(v) = parse_env_var::<String>("ANNEX_RATE_LIMIT_BACKEND")? {
        config.deployment.rate_limit_backend = v;
    }
    if let Some(v) = parse_env_bool("ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED")? {
        config.deployment.experimental_relay_transport_enabled = v;
    }

    validate_config(&config)?;

    Ok(config)
}

fn generate_server_slug() -> String {
    let mut bytes = [0_u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn derive_server_slug_from_public_url(public_url: &str) -> String {
    let normalized = public_url.trim().trim_end_matches('/');
    if normalized.is_empty() {
        return generate_server_slug();
    }
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(&digest[..6])
}

fn sync_config_defaults_to_disk(
    path: &str,
    existing_contents: &str,
    config: &Config,
) -> Result<(), std::io::Error> {
    let mut root = if existing_contents.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        existing_contents
            .parse::<toml::Value>()
            .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()))
    };

    let root_table = if let Some(root_table) = root.as_table_mut() {
        root_table
    } else {
        root = toml::Value::Table(toml::map::Map::new());
        root.as_table_mut()
            .expect("just assigned TOML table root value")
    };
    let server = root_table
        .entry("server")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !server.is_table() {
        *server = toml::Value::Table(toml::map::Map::new());
    }
    let server_table = server
        .as_table_mut()
        .expect("server section ensured as table");

    if !server_table.contains_key("server_slug") {
        server_table.insert(
            "server_slug".to_string(),
            toml::Value::String(config.server.server_slug.clone()),
        );
    }
    if !server_table.contains_key("invite_base_url") {
        server_table.insert(
            "invite_base_url".to_string(),
            toml::Value::String(config.server.invite_base_url.clone()),
        );
    }
    if !server_table.contains_key("merkle_tree_depth") {
        server_table.insert(
            "merkle_tree_depth".to_string(),
            toml::Value::Integer(config.server.merkle_tree_depth as i64),
        );
    }
    if !server_table.contains_key("presence_broadcast_capacity") {
        server_table.insert(
            "presence_broadcast_capacity".to_string(),
            toml::Value::Integer(config.server.presence_broadcast_capacity as i64),
        );
    }

    let rendered = toml::to_string_pretty(&root)
        .map_err(|e| std::io::Error::other(format!("failed to render TOML: {e}")))?;
    atomic_write_file(path, &rendered)
}

fn atomic_write_file(path: &str, contents: &str) -> Result<(), std::io::Error> {
    let target = Path::new(path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");
    let tmp_name = format!(".{file_name}.tmp-{}", std::process::id());
    let tmp_path = target.with_file_name(tmp_name);

    {
        let mut tmp = std::fs::File::create(&tmp_path)?;
        use std::io::Write;
        tmp.write_all(contents.as_bytes())?;
        tmp.sync_all()?;
    }

    std::fs::rename(&tmp_path, target)?;
    Ok(())
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
        // Mirror every ANNEX_* var that load_config reads. Missing any of these
        // leaves cross-test residue that re-poisons env_lock for subsequent tests.
        for name in [
            "ANNEX_HOST",
            "ANNEX_PORT",
            "ANNEX_RETENTION_CHECK_INTERVAL_SECONDS",
            "ANNEX_IDEMPOTENCY_TTL_SECONDS",
            "ANNEX_FEDERATION_OUTBOX_PER_PEER_BATCH",
            "ANNEX_INACTIVITY_THRESHOLD_SECONDS",
            "ANNEX_PUBLIC_URL",
            "ANNEX_MERKLE_TREE_DEPTH",
            "ANNEX_PRESENCE_BROADCAST_CAPACITY",
            "ANNEX_INVITE_BASE_URL",
            "ANNEX_DB_PATH",
            "ANNEX_DB_BUSY_TIMEOUT_MS",
            "ANNEX_DB_POOL_MAX_SIZE",
            "ANNEX_LOG_LEVEL",
            "ANNEX_LOG_JSON",
            "ANNEX_WEBRTC_URL",
            "ANNEX_WEBRTC_PUBLIC_URL",
            "ANNEX_WEBRTC_API_KEY",
            "ANNEX_WEBRTC_API_SECRET",
            "ANNEX_TTS_VOICES_DIR",
            "ANNEX_TTS_BINARY_PATH",
            "ANNEX_STT_MODEL_PATH",
            "ANNEX_STT_BINARY_PATH",
            "ANNEX_BARK_BINARY_PATH",
            "ANNEX_CORS_ORIGINS",
            "ANNEX_ENFORCE_ZK_PROOFS",
            "ANNEX_BUILD_PROFILE",
            "ANNEX_TRUSTED_PROXY_DEPTH",
            "ANNEX_DEPLOYMENT_MODE",
            "ANNEX_RATE_LIMIT_BACKEND",
            "ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED",
            "ANNEX_SIGNAL_TRUSTED_PEERS",
            "ANNEX_SIGNING_KEY",
        ] {
            std::env::remove_var(name);
        }
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
        // FINDING-001: enforce_zk_proofs must default to true
        assert!(
            cfg.security.enforce_zk_proofs,
            "enforce_zk_proofs must default to true for security"
        );
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

    #[test]
    fn tiny_idempotency_ttl_returns_error() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[server]
idempotency_ttl_seconds = 5
"#,
        );

        let err = load_config(Some(path.as_str()))
            .expect_err("load should fail for sub-minimum idempotency TTL");
        match err {
            ConfigError::InvalidValue { field, .. } => {
                assert_eq!(field, "server.idempotency_ttl_seconds")
            }
            other => panic!("unexpected error: {other}"),
        }

        fs::remove_file(path).expect("failed to remove temp config");
        clear_env();
    }

    #[test]
    fn idempotency_ttl_env_overrides_config_file() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[server]
idempotency_ttl_seconds = 86400
"#,
        );

        std::env::set_var("ANNEX_IDEMPOTENCY_TTL_SECONDS", "172800");
        let cfg = load_config(Some(path.as_str())).expect("load should succeed");
        assert_eq!(cfg.server.idempotency_ttl_seconds, 172_800);

        fs::remove_file(path).expect("failed to remove temp config");
        clear_env();
    }

    #[test]
    fn idempotency_ttl_defaults_to_seven_days() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let cfg = load_config(None).expect("load should succeed");
        assert_eq!(cfg.server.idempotency_ttl_seconds, 604_800);

        clear_env();
    }

    #[test]
    fn windows_backslash_paths_are_fixed_during_load() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        // Simulate a config file generated on Windows with backslash paths.
        // TOML treats \U as a unicode escape, so this would fail without the
        // backslash fix. We write the raw bytes to avoid Rust string escaping.
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let file_name = format!("annex-config-bslash-{unique_suffix}.toml");
        let path = std::env::temp_dir().join(file_name);

        // Write raw bytes: the file on disk literally contains C:\Users\...
        let raw_content = b"[server]\nhost = \"127.0.0.1\"\nport = 3000\n\n[database]\npath = \"C:\\Users\\monty\\AppData\\Roaming\\Annex\\annex.db\"\nbusy_timeout_ms = 5000\npool_max_size = 8\n\n[logging]\nlevel = \"info\"\njson = false\n";
        fs::write(&path, raw_content).expect("failed to write temp config with backslashes");

        // Verify the raw file actually contains backslashes
        let before = fs::read_to_string(&path).expect("should read");
        assert!(
            before.contains('\\'),
            "test setup: file should contain backslashes"
        );

        let path_str = path.to_string_lossy().into_owned();
        let cfg =
            load_config(Some(&path_str)).expect("load should succeed despite backslash paths");
        assert_eq!(
            cfg.database.path,
            "C:/Users/monty/AppData/Roaming/Annex/annex.db"
        );

        // Verify the file on disk was also fixed
        let after = fs::read_to_string(&path).expect("should read fixed file");
        assert!(
            !after.contains('\\'),
            "on-disk config should have forward slashes after load"
        );
        assert!(after.contains("C:/Users/monty/AppData/Roaming/Annex/annex.db"));

        fs::remove_file(path).expect("failed to remove temp config");
    }

    #[test]
    fn forward_slash_paths_are_not_modified() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        // The fixture pins `server_slug` so the slug-autogen path in
        // `load_config` does not rewrite the file; that lets us assert the
        // narrow invariant this test owns: forward-slash paths trigger no I/O.
        let original = r#"
[server]
host = "127.0.0.1"
port = 3000
server_slug = "fixture00001"

[database]
path = "C:/Users/monty/AppData/Roaming/Annex/annex.db"
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = "info"
json = false
"#;
        let path = write_temp_config(original);

        let cfg = load_config(Some(path.as_str())).expect("load should succeed");
        assert_eq!(
            cfg.database.path,
            "C:/Users/monty/AppData/Roaming/Annex/annex.db"
        );

        // File should be unchanged (no unnecessary writes)
        let after = fs::read_to_string(&path).expect("should read file");
        assert_eq!(after, original);

        fs::remove_file(path).expect("failed to remove temp config");
    }

    #[test]
    fn multiple_backslash_paths_are_all_fixed() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let file_name = format!("annex-config-multi-{unique_suffix}.toml");
        let path = std::env::temp_dir().join(file_name);

        // Config with multiple Windows paths. `server_slug` is pre-set so the
        // only rewrite this test exercises is the backslash-fix path; the
        // slug-autogen path is inert for fixture stability.
        let raw = b"[server]\nserver_slug = \"fixture00002\"\n\n[database]\npath = \"D:\\Servers\\Annex\\data\\annex.db\"\nbusy_timeout_ms = 5000\npool_max_size = 8\n\n[voice]\ntts_voices_dir = \"D:\\Servers\\Annex\\voices\"\ntts_binary_path = \"D:\\Servers\\Annex\\piper\\piper.exe\"\nstt_model_path = \"D:\\Servers\\Annex\\models\\ggml-base.en.bin\"\nstt_binary_path = \"D:\\Servers\\Annex\\whisper\\whisper.exe\"\n\n[logging]\nlevel = \"info\"\njson = false\n";
        fs::write(&path, raw).expect("failed to write");

        let path_str = path.to_string_lossy().into_owned();
        let cfg = load_config(Some(&path_str))
            .expect("load should succeed with multiple backslash paths");

        assert_eq!(cfg.database.path, "D:/Servers/Annex/data/annex.db");
        assert_eq!(cfg.voice.tts_voices_dir, "D:/Servers/Annex/voices");
        assert_eq!(
            cfg.voice.tts_binary_path,
            "D:/Servers/Annex/piper/piper.exe"
        );

        let after = fs::read_to_string(&path).expect("should read");
        assert!(!after.contains('\\'));

        fs::remove_file(path).expect("failed to remove temp config");
    }

    #[test]
    fn generates_and_persists_server_slug_when_missing() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();

        let path = write_temp_config(
            r#"
[server]
host = "127.0.0.1"
port = 3000
"#,
        );

        let cfg = load_config(Some(path.as_str())).expect("load should succeed");
        assert_eq!(cfg.server.server_slug.len(), 12);
        assert!(cfg
            .server
            .server_slug
            .chars()
            .all(|c| c.is_ascii_hexdigit()));

        let persisted = fs::read_to_string(&path).expect("must re-read config");
        assert!(persisted.contains("server_slug"));
        assert!(persisted.contains(&cfg.server.server_slug));

        fs::remove_file(path).expect("failed to remove temp config");
    }

    // ── Production CORS gate ────────────────────────────────────────────
    //
    // These tests exercise the validate_cors_for_build_profile branch
    // (called from validate_config). The gate is keyed on the runtime
    // ANNEX_BUILD_PROFILE env var, so each test sets it explicitly and
    // relies on clear_env() to wipe it afterwards via the next call.

    #[test]
    fn production_profile_rejects_wildcard_cors() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        std::env::set_var("ANNEX_CORS_ORIGINS", "*");

        let err = load_config(None).expect_err("production + wildcard CORS must fail validation");
        match err {
            ConfigError::InvalidValue { field, reason } => {
                assert_eq!(field, "cors.allowed_origins");
                assert!(
                    reason.contains("wildcard"),
                    "unexpected reason text: {reason}"
                );
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn production_profile_rejects_empty_cors() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        // No ANNEX_CORS_ORIGINS, no [cors] section → empty list.

        let err = load_config(None).expect_err("production + empty CORS must fail validation");
        match err {
            ConfigError::InvalidValue { field, reason } => {
                assert_eq!(field, "cors.allowed_origins");
                assert!(
                    reason.contains("no CORS allowed origins"),
                    "unexpected reason text: {reason}"
                );
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn production_profile_accepts_explicit_origins() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        std::env::set_var(
            "ANNEX_CORS_ORIGINS",
            "https://app.example.com,https://admin.example.com",
        );

        let cfg = load_config(None).expect("explicit origins must validate under production");
        assert_eq!(
            cfg.cors.allowed_origins,
            vec![
                "https://app.example.com".to_string(),
                "https://admin.example.com".to_string()
            ]
        );
    }

    #[test]
    fn release_profile_alias_also_gated() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "release");
        std::env::set_var("ANNEX_CORS_ORIGINS", "*");

        let err = load_config(None).expect_err("release alias must reject wildcard CORS");
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn dev_profile_allows_wildcard_cors() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "dev");
        std::env::set_var("ANNEX_CORS_ORIGINS", "*");

        let cfg = load_config(None).expect("dev profile must allow wildcard CORS");
        assert_eq!(cfg.cors.allowed_origins, vec!["*".to_string()]);
    }

    #[test]
    fn unset_profile_allows_wildcard_cors() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        // ANNEX_BUILD_PROFILE intentionally unset — current behaviour for
        // cargo run, plain tests, etc. The gate must stay out of the way.
        std::env::set_var("ANNEX_CORS_ORIGINS", "*");

        let cfg = load_config(None)
            .expect("absent ANNEX_BUILD_PROFILE must not trip the production CORS gate");
        assert_eq!(cfg.cors.allowed_origins, vec!["*".to_string()]);
    }

    // ── Deployment topology gates ────────────────────────────────────────

    #[test]
    fn production_clustered_with_memory_backend_is_rejected() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        std::env::set_var("ANNEX_CORS_ORIGINS", "https://app.example.com");
        std::env::set_var("ANNEX_DEPLOYMENT_MODE", "clustered");
        // Memory is the default; setting it explicitly here documents intent.
        std::env::set_var("ANNEX_RATE_LIMIT_BACKEND", "memory");

        let err = load_config(None).expect_err("production+clustered+memory must fail at startup");
        match err {
            ConfigError::InvalidValue { field, reason } => {
                assert_eq!(field, "deployment.rate_limit_backend");
                assert!(reason.contains("clustered"), "unexpected reason: {reason}");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn dev_clustered_with_memory_is_allowed() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "dev");
        std::env::set_var("ANNEX_DEPLOYMENT_MODE", "clustered");
        std::env::set_var("ANNEX_RATE_LIMIT_BACKEND", "memory");

        // Dev tolerates the in-memory mismatch with a warning at runtime.
        let cfg = load_config(None).expect("dev must allow clustered+memory");
        assert_eq!(cfg.deployment.mode, "clustered");
    }

    #[test]
    fn production_rejects_unimplemented_rate_limit_backend() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        std::env::set_var("ANNEX_CORS_ORIGINS", "https://app.example.com");
        std::env::set_var("ANNEX_DEPLOYMENT_MODE", "single");
        std::env::set_var("ANNEX_RATE_LIMIT_BACKEND", "redis");

        let err = load_config(None).expect_err("unimplemented backend must fail");
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn unknown_deployment_mode_is_rejected() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_DEPLOYMENT_MODE", "highly-available");

        let err = load_config(None).expect_err("unknown mode must fail");
        match err {
            ConfigError::InvalidValue { field, .. } => {
                assert_eq!(field, "deployment.mode");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn trusted_proxy_depth_capped_at_16() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_TRUSTED_PROXY_DEPTH", "99");

        let err = load_config(None).expect_err("over-cap depth must fail");
        match err {
            ConfigError::InvalidValue { field, .. } => {
                assert_eq!(field, "deployment.trusted_proxy_depth");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn production_relay_transport_requires_trust_map() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        std::env::set_var("ANNEX_CORS_ORIGINS", "https://app.example.com");
        std::env::set_var("ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED", "true");
        // No ANNEX_SIGNAL_TRUSTED_PEERS → reject.

        let err = load_config(None).expect_err("relay transport without trust map must fail");
        match err {
            ConfigError::InvalidValue { field, reason } => {
                assert_eq!(field, "deployment.experimental_relay_transport_enabled");
                assert!(
                    reason.contains("ANNEX_SIGNAL_TRUSTED_PEERS"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn production_relay_transport_with_trust_map_accepted() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "production");
        std::env::set_var("ANNEX_CORS_ORIGINS", "https://app.example.com");
        std::env::set_var("ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED", "true");
        std::env::set_var(
            "ANNEX_SIGNAL_TRUSTED_PEERS",
            "abcdef012345:cafef00dcafef00dcafef00dcafef00dcafef00dcafef00dcafef00dcafef00d",
        );

        let cfg = load_config(None).expect("trust map present → accept");
        assert!(cfg.deployment.experimental_relay_transport_enabled);
    }

    #[test]
    fn dev_relay_transport_allowed_without_trust_map() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        clear_env();
        std::env::set_var("ANNEX_BUILD_PROFILE", "dev");
        std::env::set_var("ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED", "true");

        let cfg = load_config(None).expect("dev allows relay transport without trust map");
        assert!(cfg.deployment.experimental_relay_transport_enabled);
    }
}
