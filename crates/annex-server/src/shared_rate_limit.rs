//! Shared admission across server processes. Redis owns the clock and executes
//! the counter transition atomically. An unavailable store never grants a local
//! fallback budget. Keys contain digests, not client IPs or pseudonyms.

use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use sha2::{Digest, Sha256};

use crate::middleware::{RateLimitCategory, RateLimitKey, RATE_LIMIT_WINDOW_SECS};

const DEADLINE: Duration = Duration::from_secs(2);

// Keep each bucket in one Redis key. This is atomic on a single primary and
// works with namespace isolation without requiring cross-slot operations.
// Invalid/corrupted state causes a Redis error and therefore a closed gate.
const ADMIT: &str = r#"
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local window = tonumber(ARGV[1])
local limit = tonumber(ARGV[2])
local values = redis.call('HMGET', KEYS[1], 'start', 'previous', 'current')
local start, previous, current = now, 0, 0
if values[1] or values[2] or values[3] then
  start = tonumber(values[1])
  previous = tonumber(values[2])
  current = tonumber(values[3])
  if not start or not previous or not current or start < 0 or previous < 0 or current < 0 or previous > 4294967296 or current > 4294967296 then
    return redis.error_reply('invalid admission state')
  end
end
-- A backwards clock cannot grant a fresh window. Preserve the prior budget.
now = math.max(now, start)
local elapsed = now - start
if elapsed >= window * 2 then
  start, previous, current = now, 0, 0
elseif elapsed >= window then
  start, previous, current = start + window, current, 0
end
current = math.min(current + 1, 4294967296)
redis.call('HSET', KEYS[1], 'start', start, 'previous', previous, 'current', current)
redis.call('PEXPIRE', KEYS[1], window * 2)
local effective = previous * (1 - math.min((now - start) / window, 1)) + current
if effective <= limit then return 1 end
return 0
"#;

/// Content-free errors: Redis endpoints can contain credentials, and server
/// error text is not safe to echo into operator or request logs.
#[derive(Debug, thiserror::Error)]
pub enum SharedRateLimitError {
    #[error("invalid shared rate-limit configuration")]
    Configuration,
    #[error("shared rate-limit store unavailable; request admission refused")]
    Unavailable,
    #[error("shared rate-limit store returned invalid admission state")]
    InvalidState,
}

#[derive(Clone)]
pub struct SharedRateLimiter {
    connection: ConnectionManager,
    prefix: String,
    script: redis::Script,
}

impl std::fmt::Debug for SharedRateLimiter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedRateLimiter")
            .finish_non_exhaustive()
    }
}

impl SharedRateLimiter {
    pub async fn connect(endpoint: &str, namespace: &str) -> Result<Self, SharedRateLimitError> {
        validate_configuration(endpoint, namespace)?;
        let client =
            redis::Client::open(endpoint).map_err(|_| SharedRateLimitError::Configuration)?;
        let settings = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(DEADLINE))
            .set_response_timeout(Some(DEADLINE))
            .set_number_of_retries(1);
        let connection = tokio::time::timeout(
            DEADLINE,
            ConnectionManager::new_with_config(client, settings),
        )
        .await
        .map_err(|_| SharedRateLimitError::Unavailable)?
        .map_err(|_| SharedRateLimitError::Unavailable)?;
        let limiter = Self {
            connection,
            prefix: format!(
                "annex:admission:v1:{}:",
                hex::encode(Sha256::digest(namespace.as_bytes()))
            ),
            script: redis::Script::new(ADMIT),
        };
        // Verify actual scripting permissions before the listener is opened.
        // This probe uses a unique expiring key, never an application bucket.
        let probe = RateLimitKey::Pseudonym(
            format!("startup:{}", uuid::Uuid::new_v4()),
            RateLimitCategory::Probe,
        );
        if limiter.check(&probe, 0).await? {
            return Err(SharedRateLimitError::InvalidState);
        }
        Ok(limiter)
    }

    pub async fn check(
        &self,
        key: &RateLimitKey,
        limit: u32,
    ) -> Result<bool, SharedRateLimitError> {
        let mut connection = self.connection.clone();
        let mut invocation = self.script.prepare_invoke();
        invocation
            .key(self.storage_key(key))
            .arg(RATE_LIMIT_WINDOW_SECS * 1000)
            .arg(limit);
        let result: i64 = tokio::time::timeout(DEADLINE, invocation.invoke_async(&mut connection))
            .await
            .map_err(|_| SharedRateLimitError::Unavailable)?
            .map_err(|_| SharedRateLimitError::Unavailable)?;
        match result {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(SharedRateLimitError::InvalidState),
        }
    }

    fn storage_key(&self, key: &RateLimitKey) -> String {
        let (kind, identity, category) = match key {
            RateLimitKey::Ip(ip, category) => ("ip", ip.to_string(), category),
            RateLimitKey::Pseudonym(id, category) => ("pseudonym", id.clone(), category),
        };
        let category = match category {
            RateLimitCategory::Default => "default",
            RateLimitCategory::Registration => "registration",
            RateLimitCategory::Verification => "verification",
            RateLimitCategory::Probe => "probe",
        };
        let mut hash = Sha256::new();
        hash.update(kind);
        hash.update([0]);
        hash.update(category);
        hash.update([0]);
        hash.update(identity);
        format!("{}{}", self.prefix, hex::encode(hash.finalize()))
    }
}

pub fn validate_configuration(endpoint: &str, namespace: &str) -> Result<(), SharedRateLimitError> {
    let parsed = url::Url::parse(endpoint).map_err(|_| SharedRateLimitError::Configuration)?;
    if !matches!(parsed.scheme(), "redis" | "rediss")
        || parsed.host_str().is_none()
        || parsed.fragment().is_some()
        || namespace.trim().is_empty()
        || namespace.len() > 128
        || namespace != namespace.trim()
    {
        return Err(SharedRateLimitError::Configuration);
    }
    // #insecure disables TLS verification in redis-rs; fragments are rejected
    // above. Certificate validation stays on for every rediss connection.
    redis::Client::open(endpoint).map_err(|_| SharedRateLimitError::Configuration)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_never_in_configuration_errors() {
        let endpoint = "https://operator:secret@example.test";
        let error = validate_configuration(endpoint, "cluster").unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret"));
        assert!(validate_configuration("rediss://example.test/#insecure", "cluster").is_err());
        assert!(validate_configuration("redis://127.0.0.1/", " ").is_err());
        assert!(validate_configuration("rediss://user:secret@example.test/0", "cluster").is_ok());
    }
}
