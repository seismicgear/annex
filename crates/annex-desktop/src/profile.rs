//! Explicit, isolated desktop profiles for independent native sessions.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_IDENTIFIER: &str = "com.annex.desktop";
const MARKER: &str = ".annex-profile.json";

/// Tauri's updater and restart API inherit its managed argv. Preserve only
/// executable/profile selection; never replay bearer invite links or pass them
/// to installer command-line logs. Deep-link delivery still reads OS argv.
pub(crate) fn restart_environment(
    args: impl IntoIterator<Item = OsString>,
) -> Result<tauri::Env, String> {
    let mut args = args.into_iter();
    let executable = args.next().ok_or("missing executable argument")?;
    let profile = profile_argument(args)?;
    let mut environment = tauri::Env::default();
    environment.args_os = vec![executable];
    if let Some(profile) = profile {
        environment.args_os.push("--profile-dir".into());
        environment.args_os.push(profile.into_os_string());
    }
    Ok(environment)
}

#[derive(Debug)]
pub(crate) struct DesktopProfile {
    pub(crate) data_dir: PathBuf,
    pub(crate) identifier: String,
    isolated: Option<(PathBuf, uuid::Uuid)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileMarker {
    version: u32,
    id: uuid::Uuid,
}

pub(crate) fn profile_argument(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Option<PathBuf>, String> {
    let mut args = args.into_iter();
    let mut chosen = None;
    while let Some(arg) = args.next() {
        let path = if arg == "--profile-dir" {
            Some(
                args.next()
                    .ok_or("--profile-dir requires an absolute directory")?,
            )
        } else {
            arg.to_str()
                .and_then(|s| s.strip_prefix("--profile-dir="))
                .map(OsString::from)
        };
        if let Some(path) = path {
            if chosen.is_some() {
                return Err("--profile-dir may only be supplied once".into());
            }
            let path = PathBuf::from(path);
            if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
                return Err(
                    "--profile-dir must be absolute and contain no parent traversal".into(),
                );
            }
            chosen = Some(path);
        }
    }
    Ok(chosen)
}

// Refuse symlink/junction redirection before creating files or accepting a
// previously used profile. Windows junctions are reparse points, not symlinks.
fn check_no_redirects(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                let redirected = metadata.file_type().is_symlink();
                #[cfg(windows)]
                let redirected = {
                    use std::os::windows::fs::MetadataExt;
                    redirected || metadata.file_attributes() & 0x400 != 0
                };
                if redirected {
                    return Err(format!(
                        "isolated profile cannot use a redirected path: {}",
                        ancestor.display()
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "cannot inspect profile path {}: {e}",
                    ancestor.display()
                ))
            }
        }
    }
    Ok(())
}

/// Compare paths even before their final directories exist. Resolving the
/// nearest existing ancestor also catches Windows short-name/canonical aliases.
fn projected_canonical(path: &Path) -> Result<PathBuf, String> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for part in missing.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let part = ancestor
                    .file_name()
                    .ok_or("profile path has no resolvable ancestor")?;
                missing.push(part.to_os_string());
                ancestor = ancestor.parent().ok_or("profile path has no parent")?;
            }
            Err(error) => return Err(format!("cannot resolve profile boundary: {error}")),
        }
    }
}

fn comparable_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let text = text
        .strip_prefix("unc/")
        .map(|tail| format!("//{tail}"))
        .unwrap_or_else(|| text.to_owned());
    text.trim_end_matches('/').to_owned()
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = comparable_path(left);
    let right = comparable_path(right);
    left == right
        || left.starts_with(&format!("{right}/"))
        || right.starts_with(&format!("{left}/"))
}

fn validate_profile_boundaries(root: &Path, reserved: &[PathBuf]) -> Result<(), String> {
    check_no_redirects(root)?;
    let canonical_root = projected_canonical(root)?;
    for directory in reserved {
        if paths_overlap(root, directory)
            || paths_overlap(&canonical_root, &projected_canonical(directory)?)
        {
            return Err("isolated profile must be separate from every default Annex data and WebView directory".into());
        }
    }
    Ok(())
}

fn default_uninstall_trees(default_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![default_dir.to_path_buf()];
    if let Some(local) = dirs::data_local_dir() {
        paths.push(local.join("Annex"));
        paths.push(local.join(DEFAULT_IDENTIFIER));
    }
    paths
}

impl DesktopProfile {
    pub(crate) fn resolve(
        requested: Option<PathBuf>,
        default_dir: PathBuf,
    ) -> Result<Self, String> {
        let Some(root) = requested else {
            return Ok(Self {
                data_dir: default_dir,
                identifier: DEFAULT_IDENTIFIER.into(),
                isolated: None,
            });
        };
        if !root.is_absolute() || root.components().any(|c| c == Component::ParentDir) {
            return Err("isolated profile path must be absolute without parent traversal".into());
        }
        let reserved = default_uninstall_trees(&default_dir);
        validate_profile_boundaries(&root, &reserved)?;
        let marker_path = root.join(MARKER);
        check_no_redirects(&marker_path)?;
        if !marker_path.exists() && root.exists() {
            let mut entries =
                std::fs::read_dir(&root).map_err(|e| format!("cannot inspect profile: {e}"))?;
            if entries.next().is_some() {
                return Err("a new isolated profile requires an empty directory".into());
            }
        }
        std::fs::create_dir_all(&root)
            .map_err(|e| format!("cannot create isolated profile: {e}"))?;
        validate_profile_boundaries(&root, &reserved)?;
        let marker_path = root.join(MARKER);
        let marker = if marker_path.exists() {
            let bytes = std::fs::read(&marker_path)
                .map_err(|e| format!("cannot read profile identity: {e}"))?;
            serde_json::from_slice::<ProfileMarker>(&bytes)
                .map_err(|e| format!("invalid profile identity: {e}"))?
        } else {
            let marker = ProfileMarker {
                version: 1,
                id: uuid::Uuid::new_v4(),
            };
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker_path)
                .map_err(|e| format!("cannot create profile identity: {e}"))?;
            file.write_all(&serde_json::to_vec(&marker).map_err(|e| e.to_string())?)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("cannot persist profile identity: {e}"))?;
            marker
        };
        if marker.version != 1 || marker.id.get_version_num() != 4 {
            return Err("unsupported isolated profile identity".into());
        }
        for name in [
            "server",
            "server/config.toml",
            "server/annex.db",
            "server/uploads",
            "webview",
        ] {
            check_no_redirects(&root.join(name))?;
        }
        Ok(Self {
            data_dir: root.join("server"),
            identifier: format!("{DEFAULT_IDENTIFIER}.profile.p{}", marker.id.simple()),
            isolated: Some((root, marker.id)),
        })
    }

    pub(crate) fn is_isolated(&self) -> bool {
        self.isolated.is_some()
    }

    pub(crate) fn webview_dir(&self, label: &str) -> Result<PathBuf, String> {
        if label.is_empty()
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid isolated webview label".into());
        }
        let (root, _) = self.isolated.as_ref().ok_or("profile is not isolated")?;
        let path = root.join("webview").join(label);
        check_no_redirects(&path)?;
        Ok(path)
    }

    pub(crate) fn configure(&self, config: &mut tauri::Config) {
        if let Some((_, id)) = &self.isolated {
            config.identifier.clone_from(&self.identifier);
            config.plugins.0.remove("deep-link");
            config.plugins.0.remove("updater");
            for window in &mut config.app.windows {
                // WindowConfig only permits relative paths. Build the isolated
                // window explicitly with an absolute data_directory in setup.
                window.create = false;
                window.data_directory = None;
                window.data_store_identifier = Some(*id.as_bytes());
                window.title =
                    format!("Annex — Isolated profile {}", &id.simple().to_string()[..8]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_default_uninstall_trees_are_reserved_before_creation() {
        let dir = tempfile::tempdir().unwrap();
        let reserved = vec![
            dir.path().join("roaming/Annex"),
            dir.path().join("local/Annex"),
            dir.path().join("local/com.annex.desktop"),
        ];
        for tree in &reserved {
            for candidate in [
                tree.clone(),
                tree.join("isolated/new"),
                tree.parent().unwrap().to_path_buf(),
            ] {
                assert!(validate_profile_boundaries(&candidate, &reserved).is_err());
                assert!(!candidate.exists());
            }
        }
        let isolated = dir.path().join("independent/new");
        assert!(validate_profile_boundaries(&isolated, &reserved).is_ok());
        assert!(!isolated.exists());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn canonical_comparison_handles_missing_children_and_windows_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let reserved = dir.path().join("Annex");
        let candidate = dir.path().join(".").join("Annex/child");
        assert!(validate_profile_boundaries(&candidate, &[reserved]).is_err());
        assert!(!candidate.exists());
        assert!(paths_overlap(
            Path::new("C:\\Users\\Name\\Annex"),
            Path::new("\\\\?\\C:\\users\\name\\annex\\child")
        ));
        assert!(!paths_overlap(
            Path::new("C:\\data\\Annex"),
            Path::new("C:\\data\\AnnexOther")
        ));
    }

    #[test]
    fn restart_retains_isolation_but_never_replays_invite_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let input = vec![
            "annex.exe".into(),
            "--profile-dir".into(),
            dir.path().as_os_str().to_owned(),
            "annex://invite?server=https://private&code=secret".into(),
        ];
        let environment = restart_environment(input).unwrap();
        assert_eq!(environment.args_os.len(), 3);
        assert_eq!(environment.args_os[2], dir.path().as_os_str());
        let normal =
            restart_environment(["annex.exe".into(), "annex://invite?code=secret".into()]).unwrap();
        assert_eq!(normal.args_os, vec![OsString::from("annex.exe")]);
    }

    #[test]
    fn explicit_paths_are_required_and_duplicates_rejected() {
        assert!(profile_argument(["--profile-dir".into()]).is_err());
        assert!(profile_argument(["--profile-dir=relative".into()]).is_err());
        let dir = tempfile::tempdir().unwrap();
        let args = [
            OsString::from("--profile-dir"),
            dir.path().as_os_str().into(),
        ];
        assert_eq!(
            profile_argument(args.clone()).unwrap(),
            Some(dir.path().to_path_buf())
        );
        assert!(profile_argument(args.into_iter().cycle().take(4)).is_err());
    }

    #[test]
    fn default_profile_is_unchanged_and_isolated_profiles_remain_distinct_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let default_dir = dir.path().join("default");
        let default = DesktopProfile::resolve(None, default_dir.clone()).unwrap();
        assert_eq!(default.identifier, DEFAULT_IDENTIFIER);
        assert_eq!(default.data_dir, default_dir);
        assert!(!default_dir.exists());
        let a = DesktopProfile::resolve(Some(dir.path().join("a")), default_dir.clone()).unwrap();
        let b = DesktopProfile::resolve(Some(dir.path().join("b")), default_dir.clone()).unwrap();
        let again =
            DesktopProfile::resolve(Some(dir.path().join("a")), default_dir.clone()).unwrap();
        assert_eq!(a.identifier, again.identifier);
        assert_ne!(a.identifier, b.identifier);
        assert_ne!(a.identifier, default.identifier); // Same namespace used by keyring and singleton.
        assert_ne!(a.data_dir, b.data_dir);
        let mut config = tauri::Config::default();
        config
            .app
            .windows
            .push(tauri::utils::config::WindowConfig::default());
        config
            .plugins
            .0
            .insert("deep-link".into(), serde_json::json!({}));
        config
            .plugins
            .0
            .insert("updater".into(), serde_json::json!({}));
        a.configure(&mut config);
        assert_eq!(config.identifier, a.identifier);
        assert!(!config.app.windows[0].create);
        assert!(a
            .webview_dir("main")
            .unwrap()
            .starts_with(a.data_dir.parent().unwrap()));
        assert_ne!(
            a.webview_dir("main").unwrap(),
            b.webview_dir("main").unwrap()
        );
        assert!(a.webview_dir("../escape").is_err());
        assert!(config.app.windows[0].data_store_identifier.is_some());
        assert!(!config.plugins.0.contains_key("deep-link"));
        assert!(!config.plugins.0.contains_key("updater"));
        assert!(!default_dir.exists());
    }

    #[test]
    fn existing_unmarked_data_default_tree_and_invalid_markers_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("default");
        assert!(DesktopProfile::resolve(Some(default.clone()), default.clone()).is_err());
        assert!(DesktopProfile::resolve(Some(default.join("test")), default.clone()).is_err());
        assert!(DesktopProfile::resolve(Some(dir.path().to_path_buf()), default.clone()).is_err());
        let used = dir.path().join("used");
        std::fs::create_dir(&used).unwrap();
        std::fs::write(used.join("important"), b"keep").unwrap();
        assert!(DesktopProfile::resolve(Some(used.clone()), default.clone()).is_err());
        std::fs::write(used.join(MARKER), b"{}").unwrap();
        assert!(DesktopProfile::resolve(Some(used.clone()), default).is_err());
        assert_eq!(std::fs::read(used.join("important")).unwrap(), b"keep");
    }
}
