//! Portable process fixtures for protocol tests, independent of speech models.
pub fn python_process(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(dir.join(format!("{name}.py")), source).unwrap();
        std::fs::write(
            &path,
            format!("@echo off\r\npython \"%~dp0{name}.py\" %*\r\n"),
        )
        .unwrap();
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/usr/bin/env python3\n{source}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
