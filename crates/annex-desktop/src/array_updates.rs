//! Array owns replacement of installations it manages. These local hints are
//! update ownership and diagnostics only: they grant no account, product,
//! license, channel, or download authority.

use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub(crate) const UPDATE_URI: &str = "ki-array://updates/annex";
const MARKER: &str = "array-managed-installation.json";
const MAX_MARKER_BYTES: u64 = 4096;
const INVALID_OWNER: &str = "The saved update manager could not be read. Open Annex from Array to restore its update information.";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedInstallation {
    schema_version: u32,
    product_id: String,
    manager: String,
    pub(crate) build_id: Option<String>,
    pub(crate) release_channel: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum UpdateOwner {
    Standalone,
    Array(ManagedInstallation),
    Unavailable,
}

fn build_id_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn channel_valid(value: &str) -> bool {
    matches!(value, "employee" | "beta" | "stable")
}

impl ManagedInstallation {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1
            || self.product_id != "annex"
            || self.manager != "array"
            || self
                .build_id
                .as_deref()
                .is_some_and(|value| !build_id_valid(value))
            || self
                .release_channel
                .as_deref()
                .is_some_and(|value| !channel_valid(value))
        {
            return Err(INVALID_OWNER);
        }
        Ok(())
    }
}

impl UpdateOwner {
    pub(crate) fn from_environment(data_dir: &Path, isolated: bool) -> Self {
        Self::resolve(data_dir, isolated, |name| std::env::var(name).ok())
    }

    fn resolve(
        data_dir: &Path,
        isolated: bool,
        environment: impl Fn(&str) -> Option<String>,
    ) -> Self {
        // Isolated profiles never install updates and must not change the
        // regular profile's ownership or inherit a launcher hint.
        if isolated {
            return Self::Standalone;
        }
        let load = || -> Result<Option<ManagedInstallation>, &'static str> {
            let marker = data_dir.join(MARKER);
            let existing = match std::fs::symlink_metadata(&marker) {
                Ok(metadata) => {
                    if !metadata.is_file()
                        || metadata.file_type().is_symlink()
                        || metadata.len() > MAX_MARKER_BYTES
                    {
                        return Err(INVALID_OWNER);
                    }
                    Some(marker.clone())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err(INVALID_OWNER),
            };
            if environment("KI_ARRAY_MANAGED_INSTALL").as_deref() == Some("1") {
                if environment("KI_ARRAY_PRODUCT_ID").as_deref() != Some("annex") {
                    return Err(INVALID_OWNER);
                }
                let installation = ManagedInstallation {
                    schema_version: 1,
                    product_id: "annex".into(),
                    manager: "array".into(),
                    build_id: environment("KI_ARRAY_BUILD_ID").filter(|value| !value.is_empty()),
                    release_channel: environment("KI_ARRAY_RELEASE_CHANNEL")
                        .filter(|value| !value.is_empty()),
                };
                installation.validate()?;
                save(data_dir, &marker, &installation)?;
                return Ok(Some(installation));
            }
            let Some(marker) = existing else {
                return Ok(None);
            };
            let mut bytes = Vec::new();
            std::fs::File::open(marker)
                .map_err(|_| INVALID_OWNER)?
                .take(MAX_MARKER_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| INVALID_OWNER)?;
            if bytes.len() as u64 > MAX_MARKER_BYTES {
                return Err(INVALID_OWNER);
            }
            let installation: ManagedInstallation =
                serde_json::from_slice(&bytes).map_err(|_| INVALID_OWNER)?;
            installation.validate()?;
            Ok(Some(installation))
        };
        match load() {
            Ok(Some(installation)) => Self::Array(installation),
            Ok(None) => Self::Standalone,
            Err(_) => Self::Unavailable,
        }
    }

    pub(crate) fn allows_native_updates(&self) -> bool {
        matches!(self, Self::Standalone)
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Standalone => "annex",
            Self::Array(_) => "array",
            Self::Unavailable => "unavailable",
        }
    }

    pub(crate) fn message(&self) -> Option<&'static str> {
        match self {
            Self::Standalone => None,
            Self::Array(_) => Some("Array manages updates for this installation. Open Array to check the release available to your account."),
            Self::Unavailable => Some(INVALID_OWNER),
        }
    }

    pub(crate) fn installation(&self) -> Option<&ManagedInstallation> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn require_array(&self) -> Result<(), String> {
        if matches!(self, Self::Array(_)) {
            Ok(())
        } else {
            Err("This installation is not managed by Array.".into())
        }
    }
}

fn save(data_dir: &Path, marker: &Path, value: &ManagedInstallation) -> Result<(), &'static str> {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| INVALID_OWNER)?
        .as_nanos();
    let temporary = data_dir.join(format!(".{MARKER}.{}.{suffix}", std::process::id()));
    let write = || -> Result<(), &'static str> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|_| INVALID_OWNER)?;
        let bytes = serde_json::to_vec(value).map_err(|_| INVALID_OWNER)?;
        file.write_all(&bytes).map_err(|_| INVALID_OWNER)?;
        file.sync_all().map_err(|_| INVALID_OWNER)?;
        drop(file);
        std::fs::rename(&temporary, marker).map_err(|_| INVALID_OWNER)
    };
    let result = write();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// The webview supplies no URI, program, arguments, or channel. Array resolves
/// current account eligibility after handling this fixed navigation request.
pub(crate) fn open_array() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", UPDATE_URI]);
        command.creation_flags(0x08000000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(UPDATE_URI);
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(UPDATE_URI);
        command
    };
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| {
            "Array could not be opened. Open Array and select Annex in your library.".to_owned()
        })?;
    if status.success() {
        Ok(())
    } else {
        Err("Array could not be opened. Open Array and select Annex in your library.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "annex-array-owner-{}-{stamp}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir(&directory).unwrap();
            Self(directory)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn hints(name: &str) -> Option<String> {
        match name {
            "KI_ARRAY_MANAGED_INSTALL" => Some("1".into()),
            "KI_ARRAY_PRODUCT_ID" => Some("annex".into()),
            "KI_ARRAY_BUILD_ID" => Some("annex-0.1.4-aabbcc".into()),
            "KI_ARRAY_RELEASE_CHANNEL" => Some("beta".into()),
            _ => None,
        }
    }

    #[test]
    fn managed_ownership_survives_direct_launch_and_does_not_authorize_an_update() {
        let directory = Directory::new();
        let first = UpdateOwner::resolve(&directory.0, false, hints);
        assert!(!first.allows_native_updates());
        first.require_array().unwrap();
        let direct = UpdateOwner::resolve(&directory.0, false, |_| None);
        assert!(!direct.allows_native_updates());
        assert_eq!(
            direct.installation().unwrap().release_channel.as_deref(),
            Some("beta")
        );
        assert_eq!(
            direct.installation().unwrap(),
            first.installation().unwrap()
        );
        assert_eq!(UPDATE_URI, "ki-array://updates/annex");
    }

    #[test]
    fn unmanaged_and_isolated_launches_do_not_claim_ownership() {
        let directory = Directory::new();
        let standalone = UpdateOwner::resolve(&directory.0, false, |_| None);
        assert!(standalone.allows_native_updates());
        assert!(standalone.require_array().is_err());
        let isolated = UpdateOwner::resolve(&directory.0, true, hints);
        assert!(isolated.installation().is_none());
        assert!(!directory.0.join(MARKER).exists());
    }

    #[test]
    fn invalid_or_unreadable_ownership_never_falls_back_to_public_updates() {
        for bad in [
            "{",
            "{}",
            "{\"schemaVersion\":1,\"productId\":\"ki-code\",\"manager\":\"array\"}",
        ] {
            let directory = Directory::new();
            std::fs::write(directory.0.join(MARKER), bad).unwrap();
            let owner = UpdateOwner::resolve(&directory.0, false, |_| None);
            assert!(!owner.allows_native_updates());
            assert_eq!(owner.label(), "unavailable");
        }
        let directory = Directory::new();
        std::fs::create_dir(directory.0.join(MARKER)).unwrap();
        assert!(!UpdateOwner::resolve(&directory.0, false, hints).allows_native_updates());
    }

    #[test]
    fn invalid_product_channel_and_build_hints_are_refused() {
        for (key, value) in [
            ("KI_ARRAY_PRODUCT_ID", "forge"),
            ("KI_ARRAY_RELEASE_CHANNEL", "employee&token=secret"),
            ("KI_ARRAY_BUILD_ID", "https://private/?token=secret"),
        ] {
            let directory = Directory::new();
            let owner = UpdateOwner::resolve(&directory.0, false, |name| {
                if name == key {
                    Some(value.into())
                } else {
                    hints(name)
                }
            });
            assert_eq!(owner.label(), "unavailable");
            assert!(!owner.allows_native_updates());
            assert!(!directory.0.join(MARKER).exists());
            assert!(!owner.message().unwrap().contains(value));
        }
    }

    #[test]
    fn valid_managed_launch_repairs_corrupt_metadata_without_changing_other_data() {
        let directory = Directory::new();
        std::fs::write(directory.0.join(MARKER), "broken").unwrap();
        std::fs::write(
            directory.0.join("identity.json"),
            "private identity fixture",
        )
        .unwrap();
        assert_eq!(
            UpdateOwner::resolve(&directory.0, false, hints).label(),
            "array"
        );
        assert_eq!(
            std::fs::read_to_string(directory.0.join("identity.json")).unwrap(),
            "private identity fixture"
        );
        assert_eq!(
            UpdateOwner::resolve(&directory.0, false, |_| None).label(),
            "array"
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_marker_never_follows_a_symlink() {
        let directory = Directory::new();
        let outside = directory.0.join("outside.json");
        std::fs::write(&outside, "keep").unwrap();
        std::os::unix::fs::symlink(&outside, directory.0.join(MARKER)).unwrap();
        assert_eq!(
            UpdateOwner::resolve(&directory.0, false, hints).label(),
            "unavailable"
        );
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep");
    }
}
