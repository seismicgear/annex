#[cfg(target_os = "macos")]
mod service {
    use annex_macos_updater::*;
    use std::{
        ffi::{CStr, CString},
        io,
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
        path::Path,
        sync::Mutex,
    };

    static INSTALLATION: Mutex<()> = Mutex::new(());
    extern "C" {
        fn annex_installer_service_main();
        fn annex_validate_staged_bundle(current: *const i8, staged: *const i8) -> i32;
    }

    fn string(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid bundle path"))
    }

    fn swap(target: &Path, staged: &Path) -> io::Result<()> {
        let target = string(target)?;
        let staged = string(staged)?;
        if unsafe { libc::renamex_np(staged.as_ptr(), target.as_ptr(), libc::RENAME_SWAP) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn install(bytes: &[u8], signature: &str, version: &str) -> io::Result<()> {
        let _guard = INSTALLATION
            .lock()
            .map_err(|_| io::Error::other("Installer unavailable"))?;
        verify_archive(bytes, signature, env!("ANNEX_INSTALLER_PUBLIC_KEY"))?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let target = containing_application(&executable)?;
        let metadata = std::fs::symlink_metadata(&target)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other("Application path is not a real bundle"));
        }
        let temp = std::env::temp_dir();
        let staging_root = [temp.as_path(), target.parent().unwrap()]
            .into_iter()
            .find(|path| {
                path.metadata()
                    .is_ok_and(|data| data.dev() == metadata.dev())
            })
            .ok_or_else(|| io::Error::other("No same-volume staging directory"))?;
        let workspace = tempfile::Builder::new()
            .prefix("annex-verified-update-")
            .tempdir_in(staging_root)?;
        let staged = workspace.path().join("Annex.app");
        std::fs::create_dir(&staged)?;
        extract_verified_archive(bytes, &staged)?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        validate_versions(&target, &staged, version)?;
        if unsafe {
            annex_validate_staged_bundle(string(&target)?.as_ptr(), string(&staged)?.as_ptr())
        } != 0
        {
            return Err(io::Error::other(
                "Staged application code signature rejected",
            ));
        }
        // APFS/HFS+ atomic exchange leaves a complete old or new app even if
        // the service is killed during installation. Never fall back to a
        // two-rename gap, a privileged shell or an unverified copy operation.
        // Administrator-managed/read-only installations require the normal
        // installer with the managing user's permission.
        swap(&target, &staged)
    }

    #[no_mangle]
    pub unsafe extern "C" fn annex_install_signed_archive(
        bytes: *const u8,
        length: usize,
        signature: *const i8,
        version: *const i8,
    ) -> i32 {
        // The C adapter validates XPC types and bounds before this entrypoint.
        // Never unwind across that boundary or send paths/private errors to it.
        std::panic::catch_unwind(|| {
            let bytes = std::slice::from_raw_parts(bytes, length);
            let signature = CStr::from_ptr(signature).to_str().map_err(|_| ())?;
            let version = CStr::from_ptr(version).to_str().map_err(|_| ())?;
            install(bytes, signature, version).map_err(|error| {
                eprintln!("Annex installation refused: {:?}", error.kind());
            })
        })
        .ok()
        .and_then(Result::ok)
        .map_or(2, |()| 0)
    }

    pub fn run() {
        unsafe { annex_installer_service_main() }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn atomic_exchange_keeps_both_complete_versions_and_failure_keeps_original() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("Annex.app");
            let staged = root.path().join("staged.app");
            std::fs::create_dir(&current).unwrap();
            std::fs::create_dir(&staged).unwrap();
            std::fs::write(current.join("version"), b"old").unwrap();
            std::fs::write(staged.join("version"), b"new").unwrap();
            swap(&current, &staged).unwrap();
            assert_eq!(std::fs::read(current.join("version")).unwrap(), b"new");
            assert_eq!(std::fs::read(staged.join("version")).unwrap(), b"old");
            assert!(swap(&current, &root.path().join("missing.app")).is_err());
            assert_eq!(std::fs::read(current.join("version")).unwrap(), b"new");
        }
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    service::run();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("The Annex installation service requires macOS.");
        std::process::exit(1);
    }
}
