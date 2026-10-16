#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

    fn archive(name: &str, content: &[u8]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file(name, SimpleFileOptions::default().compression_method(CompressionMethod::Stored)).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn updater_archive_extraction_preserves_the_installer_bytes() {
        let target = tempfile::tempdir().unwrap();
        let bytes = archive("annex-installer.exe", b"MZ-disposable-test-fixture");
        let mut reader = ZipArchive::new(Cursor::new(bytes)).unwrap();
        // This is the extraction API called by the pinned updater source.
        reader.extract(target.path()).unwrap();
        assert_eq!(std::fs::read(target.path().join("annex-installer.exe")).unwrap(), b"MZ-disposable-test-fixture");
    }

    #[test]
    fn an_archive_entry_cannot_escape_the_update_directory() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("update");
        std::fs::create_dir(&destination).unwrap();
        let sentinel = root.path().join("outside.exe");
        std::fs::write(&sentinel, b"preserved").unwrap();
        let mut reader = ZipArchive::new(Cursor::new(archive("../outside.exe", b"replacement"))).unwrap();
        assert!(reader.extract(&destination).is_err());
        assert_eq!(std::fs::read(sentinel).unwrap(), b"preserved");
    }
}
