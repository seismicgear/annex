//! OS keychain wrappers for sensitive desktop secrets (currently the
//! WebRTC API secret).
//!
//! All functions degrade gracefully when the platform keychain is missing or
//! refuses access — `load_api_secret_from_keyring` returns `Ok(None)` instead
//! of an error so the caller can fall back to config-file storage. The
//! intended fallback is documented next to each call site.
//!
//! The module is named `keyring` for clarity; we route through `::keyring::*`
//! with the absolute crate path so there's no ambiguity with the external
//! `keyring` crate of the same name.

const KEYRING_SERVICE: &str = "com.annex.desktop";
const KEYRING_WEBRTC_SECRET: &str = "webrtc-api-secret";

/// Store the WebRTC API secret in the OS keyring.
pub(crate) fn store_api_secret_in_keyring(secret: &str) -> Result<(), String> {
    let entry = ::keyring::Entry::new(KEYRING_SERVICE, KEYRING_WEBRTC_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring store failed: {e}"))?;
    Ok(())
}

/// Retrieve the WebRTC API secret from the OS keyring.
///
/// Returns `Ok(None)` if no secret is stored or the keyring is unavailable.
pub(crate) fn load_api_secret_from_keyring() -> Result<Option<String>, String> {
    let entry = ::keyring::Entry::new(KEYRING_SERVICE, KEYRING_WEBRTC_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(::keyring::Error::NoEntry) => Ok(None),
        Err(::keyring::Error::PlatformFailure(ref msg)) => {
            tracing::warn!("OS keyring platform failure (falling back to config): {msg}");
            Ok(None)
        }
        Err(::keyring::Error::NoStorageAccess(ref msg)) => {
            tracing::warn!("OS keyring not accessible (falling back to config): {msg}");
            Ok(None)
        }
        Err(e) => Err(format!("keyring read failed: {e}")),
    }
}

/// Delete the WebRTC API secret from the OS keyring.
pub(crate) fn delete_api_secret_from_keyring() -> Result<(), String> {
    let entry = ::keyring::Entry::new(KEYRING_SERVICE, KEYRING_WEBRTC_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(::keyring::Error::NoEntry) => Ok(()), // Already absent
        Err(e) => Err(format!("keyring delete failed: {e}")),
    }
}
