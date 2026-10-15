//! Portable child-process fixtures. These test process I/O, never speech quality.
use std::path::{Path, PathBuf};

pub(crate) fn executable_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_string()
    })
}

pub(crate) fn python_process(dir: &Path, name: &str, source: &str) -> PathBuf {
    let executable = executable_path(dir, name);
    #[cfg(windows)]
    {
        std::fs::write(dir.join(format!("{name}.py")), source).unwrap();
        std::fs::write(
            &executable,
            format!("@echo off\r\npython \"%~dp0{name}.py\" %*\r\n"),
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&executable, format!("#!/usr/bin/env python3\n{source}")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    executable
}
