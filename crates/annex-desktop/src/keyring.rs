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

const KEYRING_WEBRTC_SECRET: &str = "webrtc-api-secret";

/// Store the WebRTC API secret in the OS keyring.
pub(crate) fn store_api_secret_in_keyring(service: &str, secret: &str) -> Result<(), String> {
    let entry = ::keyring::Entry::new(service, KEYRING_WEBRTC_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring store failed: {e}"))?;
    Ok(())
}

/// Retrieve the WebRTC API secret from the OS keyring.
///
/// Returns `Ok(None)` if no secret is stored or the keyring is unavailable.
pub(crate) fn load_api_secret_from_keyring(service: &str) -> Result<Option<String>, String> {
    let entry = ::keyring::Entry::new(service, KEYRING_WEBRTC_SECRET)
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
pub(crate) fn delete_api_secret_from_keyring(service: &str) -> Result<(), String> {
    let entry = ::keyring::Entry::new(service, KEYRING_WEBRTC_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(::keyring::Error::NoEntry) => Ok(()), // Already absent
        Err(e) => Err(format!("keyring delete failed: {e}")),
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn nsis_removal_targets_only_the_default_windows_credential() {
        let credential = ::keyring::windows::WinCredential::new_with_target(
            None,
            crate::profile::DEFAULT_IDENTIFIER,
            KEYRING_WEBRTC_SECRET,
        )
        .unwrap();
        let hook = include_str!("../nsis/hooks.nsi");
        assert_eq!(
            credential.target_name,
            "webrtc-api-secret.com.annex.desktop"
        );
        assert!(hook.contains(&format!(
            "CredDeleteW(w \"{}\", i 1, i 0)",
            credential.target_name
        )));
        assert!(hook.contains("MB_DEFBUTTON2"));
        assert!(hook.contains("/SD IDNO IDNO SkipDataClean"));
        assert!(!hook.contains("com.annex.desktop.profile"));
    }

    #[test]
    fn isolated_profiles_use_independent_windows_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("unused-default");
        let a =
            crate::profile::DesktopProfile::resolve(Some(dir.path().join("a")), default.clone())
                .unwrap();
        let b =
            crate::profile::DesktopProfile::resolve(Some(dir.path().join("b")), default).unwrap();
        struct Cleanup(String, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = delete_api_secret_from_keyring(&self.0);
                let _ = delete_api_secret_from_keyring(&self.1);
            }
        }
        // These random namespaces belong only to this test. No read or write
        // touches the default desktop credential or a user's existing profile.
        let _cleanup = Cleanup(a.identifier.clone(), b.identifier.clone());
        store_api_secret_in_keyring(&a.identifier, "profile-a-test-secret").unwrap();
        assert!(load_api_secret_from_keyring(&b.identifier)
            .unwrap()
            .is_none());
        store_api_secret_in_keyring(&b.identifier, "profile-b-test-secret").unwrap();
        delete_api_secret_from_keyring(&b.identifier).unwrap();
        assert_eq!(
            load_api_secret_from_keyring(&a.identifier)
                .unwrap()
                .as_deref(),
            Some("profile-a-test-secret")
        );
        assert!(load_api_secret_from_keyring(&b.identifier)
            .unwrap()
            .is_none());
    }
}
