//! No network, caller-supplied destinations, shell commands, or release keys.
//! The XPC adapter authenticates the actual message sender before invoking this
//! code. All archive bytes are independently verified before any extraction.

use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::read::GzDecoder;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use std::{
    collections::HashSet,
    io,
    path::{Component, Path, PathBuf},
};

pub const BUNDLE_ID: &str = "com.annex.desktop";
pub const SERVICE_ID: &str = "com.annex.desktop.installer";
pub const MAX_ARCHIVE: usize = 1024 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 3 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 30_000;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Derive the sole writable app from the service's real executable location.
/// This path is never accepted over IPC or from the renderer/environment.
pub fn containing_application(executable: &Path) -> io::Result<PathBuf> {
    let suffix = Path::new("Contents")
        .join("XPCServices")
        .join(format!("{SERVICE_ID}.xpc"))
        .join("Contents/MacOS/annex-macos-updater");
    if !executable.ends_with(&suffix) {
        return Err(invalid("Installer is not inside its expected XPC service"));
    }
    let app = executable
        .ancestors()
        .nth(6)
        .ok_or_else(|| invalid("Missing containing application"))?;
    if app.extension().and_then(|value| value.to_str()) != Some("app") {
        return Err(invalid("Installer has no containing app bundle"));
    }
    Ok(app.to_path_buf())
}

pub fn verify_archive(bytes: &[u8], signature: &str, public_key: &str) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE || signature.len() > 8192 {
        return Err(invalid("Update payload is outside its size limit"));
    }
    let decode = |value: &str| -> io::Result<String> {
        String::from_utf8(
            STANDARD
                .decode(value)
                .map_err(|_| invalid("Invalid signature encoding"))?,
        )
        .map_err(|_| invalid("Invalid signature encoding"))
    };
    let key = PublicKey::decode(&decode(public_key)?)
        .map_err(|_| invalid("Installer has no valid pinned verification key"))?;
    let signature =
        Signature::decode(&decode(signature)?).map_err(|_| invalid("Invalid update signature"))?;
    key.verify(bytes, &signature, true)
        .map_err(|_| invalid("Update signature verification failed"))
}

/// Permit only one bundle root, ordinary Contents entries, and bounded regular
/// files/directories. Annex packages do not require archive links or devices.
fn archive_path(path: &Path, root: &mut Option<String>) -> io::Result<Option<PathBuf>> {
    if path.to_str().is_none_or(|name| name.contains('\\')) {
        return Err(invalid("Invalid archive name"));
    }
    let parts: Vec<_> = path.components().collect();
    let names: Vec<_> = parts
        .iter()
        .map(|part| match part {
            Component::Normal(name) => name
                .to_str()
                .filter(|name| !name.contains(['\\', ':']) && !name.is_empty())
                .ok_or_else(|| invalid("Invalid archive name")),
            _ => Err(invalid("Archive path escapes bundle")),
        })
        .collect::<Result<_, _>>()?;
    let first = names.first().ok_or_else(|| invalid("Empty archive path"))?;
    if !first.ends_with(".app") {
        return Err(invalid("Archive is not an app bundle"));
    }
    match root {
        Some(expected) if expected != first => return Err(invalid("Multiple archive roots")),
        None => *root = Some(first.to_string()),
        _ => {}
    }
    if names.len() == 1 {
        return Ok(None);
    }
    if names[1] != "Contents" {
        return Err(invalid("Unexpected bundle root entry"));
    }
    Ok(Some(names[1..].iter().collect()))
}

pub fn extract_verified_archive(bytes: &[u8], destination: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let mut root = None;
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(invalid("Too many update archive entries"));
        }
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(invalid(
                "Update archive links or special entries are forbidden",
            ));
        }
        total = total
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_EXTRACTED)
            .ok_or_else(|| invalid("Expanded update is too large"))?;
        let Some(relative) = archive_path(&entry.path()?, &mut root)? else {
            if !kind.is_dir() {
                return Err(invalid("Bundle root is not a directory"));
            }
            continue;
        };
        if !seen.insert(relative.clone()) {
            return Err(invalid("Duplicate update archive entry"));
        }
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(target)?;
    }
    if !destination.join("Contents/Info.plist").is_file()
        || !destination.join("Contents/MacOS/annex-desktop").is_file()
        || !destination
            .join("Contents/_CodeSignature/CodeResources")
            .is_file()
    {
        return Err(invalid("Update bundle is incomplete"));
    }
    Ok(())
}

fn bundle_version(bundle: &Path) -> io::Result<Version> {
    let info = plist::Value::from_file(bundle.join("Contents/Info.plist"))
        .map_err(|_| invalid("Invalid bundle metadata"))?;
    let dictionary = info
        .as_dictionary()
        .ok_or_else(|| invalid("Invalid bundle metadata"))?;
    if dictionary
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        != Some(BUNDLE_ID)
        || dictionary
            .get("CFBundleExecutable")
            .and_then(plist::Value::as_string)
            != Some("annex-desktop")
    {
        return Err(invalid("Unexpected update application identity"));
    }
    Version::parse(
        dictionary
            .get("CFBundleShortVersionString")
            .and_then(plist::Value::as_string)
            .ok_or_else(|| invalid("Missing bundle version"))?,
    )
    .map_err(|_| invalid("Invalid bundle version"))
}

pub fn validate_versions(current: &Path, staged: &Path, announced: &str) -> io::Result<()> {
    let expected = Version::parse(announced).map_err(|_| invalid("Invalid announced version"))?;
    let candidate = bundle_version(staged)?;
    if candidate != expected || candidate <= bundle_version(current)? {
        return Err(invalid(
            "Update is a downgrade, replay, or different signed version",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle(root: &Path, version: &str, id: &str) {
        std::fs::create_dir_all(root.join("Contents")).unwrap();
        let mut value = plist::Dictionary::new();
        value.insert("CFBundleIdentifier".into(), id.into());
        value.insert("CFBundleExecutable".into(), "annex-desktop".into());
        value.insert("CFBundleShortVersionString".into(), version.into());
        plist::Value::Dictionary(value)
            .to_file_xml(root.join("Contents/Info.plist"))
            .unwrap();
    }

    #[test]
    fn destination_is_derived_only_from_expected_service_layout() {
        let base = Path::new("Applications/Annex.app");
        let exe = base.join(format!(
            "Contents/XPCServices/{SERVICE_ID}.xpc/Contents/MacOS/annex-macos-updater"
        ));
        assert_eq!(containing_application(&exe).unwrap(), base);
        assert!(
            containing_application(Path::new("Applications/Other.app/annex-macos-updater"))
                .is_err()
        );
    }

    #[test]
    fn archive_rejects_escape_alternate_roots_and_unsealed_top_level_files() {
        let mut root = None;
        assert_eq!(
            archive_path(
                Path::new("Annex.app/Contents/MacOS/annex-desktop"),
                &mut root
            )
            .unwrap(),
            Some(PathBuf::from("Contents/MacOS/annex-desktop"))
        );
        for value in [
            "Other.app/Contents/a",
            "../Annex.app/Contents/a",
            "/Annex.app/Contents/a",
            "Annex.app/unsealed",
            "Annex.app/Contents/a:b",
            "Annex.app/Contents/a\\b",
            "Annex.app/Contents/../../escape",
        ] {
            assert!(
                archive_path(Path::new(value), &mut root).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn untrusted_signatures_are_rejected_before_extraction() {
        assert!(verify_archive(b"arbitrary archive", "bad-signature", "bad-key").is_err());
        assert!(verify_archive(&[], "", "").is_err());
        // Public throwaway Tauri test vector; no release signing authority.
        let key = STANDARD.encode("untrusted comment: minisign public key: A49EF4746FA6E6D7\nRWTX5qZvdPSepDGWg2bz8n6ArQnBMwgQK1geVosorcbLzcunD+KJ+cYe");
        let signature = STANDARD.encode("untrusted comment: signature from tauri secret key\nRUTX5qZvdPSepPF2zsjvP/L/TQtBLRnWRvaq+4omK1UEugtQp/USrGiI1v762sDy3id3aor7bRC9ETyeot6lMHHrWoZ5hjUM1Qc=\ntrusted comment: timestamp:1791649051\tfile:payload\tversion:2.0.0\nDDq2CLOEl5cQmC2eJRHlnZPVViOZsu1CjpPKQoei4A/XBfkDqGO0B+QMrcrHgEQ5qV3mne75tdFWhJpqUXh9AA==");
        verify_archive(b"test", &signature, &key).unwrap();
        assert!(verify_archive(b"tampered", &signature, &key).is_err());
        let changed_comment = String::from_utf8(STANDARD.decode(&signature).unwrap())
            .unwrap()
            .replace("version:2.0.0", "version:3.0.0");
        assert!(verify_archive(b"test", &STANDARD.encode(changed_comment), &key).is_err());
    }

    #[test]
    fn signed_bundle_identity_version_replay_and_downgrade_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("current.app");
        let staged = dir.path().join("staged.app");
        bundle(&current, "0.1.2", BUNDLE_ID);
        for (version, id, announced, allowed) in [
            ("0.1.3", BUNDLE_ID, "0.1.3", true),
            ("0.1.3", BUNDLE_ID, "0.1.4", false),
            ("0.1.2", BUNDLE_ID, "0.1.2", false),
            ("0.1.1", BUNDLE_ID, "0.1.1", false),
            ("0.1.3", "com.other.application", "0.1.3", false),
        ] {
            bundle(&staged, version, id);
            assert_eq!(
                validate_versions(&current, &staged, announced).is_ok(),
                allowed
            );
        }
    }

    #[test]
    fn archive_extraction_rejects_links_duplicates_and_incomplete_bundles() {
        use flate2::{write::GzEncoder, Compression};
        fn archive(names: &[(&str, tar::EntryType)]) -> Vec<u8> {
            let encoder = GzEncoder::new(Vec::new(), Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            for (path, kind) in names {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o755);
                header.set_size(if kind.is_file() { 4 } else { 0 });
                header.set_entry_type(*kind);
                if kind.is_symlink() || kind.is_hard_link() {
                    header.set_link_name("/outside").unwrap();
                }
                header.set_cksum();
                let bytes = if kind.is_file() {
                    b"test".as_slice()
                } else {
                    &[]
                };
                builder.append_data(&mut header, path, bytes).unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap()
        }
        let complete = [
            ("Annex.app/Contents/Info.plist", tar::EntryType::Regular),
            (
                "Annex.app/Contents/MacOS/annex-desktop",
                tar::EntryType::Regular,
            ),
            (
                "Annex.app/Contents/_CodeSignature/CodeResources",
                tar::EntryType::Regular,
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        extract_verified_archive(&archive(&complete), dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("Contents/MacOS/annex-desktop")).unwrap(),
            b"test"
        );
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Fifo,
            tar::EntryType::Char,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let entries = [("Annex.app/Contents/escape", kind)];
            assert!(extract_verified_archive(&archive(&entries), dir.path()).is_err());
        }
        let mut duplicate = complete.to_vec();
        duplicate.push(complete[0]);
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_verified_archive(&archive(&duplicate), dir.path()).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_verified_archive(&archive(&complete[..2]), dir.path()).is_err());
    }
}
