//! First-launch URL registration for a portable Linux AppImage.
//!
//! Installed DEBs use their package-managed desktop entry. The pinned deep-link
//! plugin instead registers the persistent AppImage path when one is present.

use std::ffi::OsStr;
use std::path::Path;

/// The upstream plugin wraps this path in double quotes without escaping it.
/// Refuse unsupported paths before it can write an invalid desktop command.
/// Ordinary Downloads paths, Unicode and spaces are supported.
fn validate_launcher_path(path: &OsStr) -> Result<(), String> {
    let message = "Annex could not enable invite links for this AppImage. Move it to an absolute path using ordinary folder and file names (spaces are supported), without quotes, backslashes, dollar signs, percent signs, backticks, equals signs or control characters, then reopen it. You can still connect to a server inside Annex.";
    let value = path.to_str().ok_or_else(|| message.to_owned())?;
    if !Path::new(value).is_absolute()
        || value.chars().any(|character| {
            character.is_control() || matches!(character, '"' | '\\' | '`' | '$' | '%' | '=')
        })
    {
        return Err(message.to_owned());
    }
    Ok(())
}

/// Invoke the supported plugin only for an AppImage, never an installed DEB.
/// The caller must keep this behind the isolated-profile guard.
pub(crate) fn register_if_appimage<E: std::fmt::Display>(
    appimage: Option<&OsStr>,
    register: impl FnOnce() -> Result<(), E>,
) -> Result<(), String> {
    let Some(path) = appimage else {
        return Ok(());
    };
    validate_launcher_path(path)?;
    register().map_err(|error| {
        format!("Annex could not enable invite links for this AppImage: {error}. Check that your user application directory is writable, then reopen the AppImage. If the error names a missing desktop utility, reinstall the official AppImage. You can still connect to a server inside Annex.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn installed_deb_does_not_replace_the_package_handler() {
        assert!(register_if_appimage::<String>(None, || panic!("must not register a DEB")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn downloads_path_with_spaces_and_unicode_registers_once() {
        let calls = Cell::new(0);
        register_if_appimage(
            Some(OsStr::new(
                "/home/test/Downloads/Annex Client – 0.1.AppImage",
            )),
            || {
                calls.set(calls.get() + 1);
                Ok::<(), String>(())
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn unsafe_interpolation_is_rejected_before_any_registration() {
        for path in [
            "relative.AppImage",
            "/tmp/Annex\".AppImage",
            "/tmp/Annex\\.AppImage",
            "/tmp/Annex`.AppImage",
            "/tmp/Annex$.AppImage",
            "/tmp/Annex%u.AppImage",
            "/tmp/Annex=.AppImage",
            "/tmp/Annex\n.AppImage",
            "/tmp/Annex\t.AppImage",
        ] {
            let result = register_if_appimage::<String>(Some(OsStr::new(path)), || {
                panic!("unsafe path must not register")
            });
            assert!(result.unwrap_err().contains("Move it"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_is_rejected_without_lossy_registration() {
        use std::os::unix::ffi::OsStrExt;
        assert!(register_if_appimage::<String>(
            Some(OsStr::from_bytes(b"/tmp/Annex\xff.AppImage")),
            || panic!("non-UTF8 path must not register")
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn utility_failure_is_actionable_and_never_reported_as_success() {
        let result = register_if_appimage(
            Some(OsStr::new("/home/test/Downloads/Annex.AppImage")),
            || Err::<(), _>("update-desktop-database failed"),
        );
        let message = result.unwrap_err();
        assert!(message.contains("update-desktop-database failed"));
        assert!(message.contains("You can still connect"));
    }
}
