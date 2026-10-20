//! A sandboxed desktop cannot replace its own executable bundle. Installation
//! runs in the narrow, authenticated XPC service sealed into that bundle.
use std::{ffi::CString, io};
use tauri_plugin_updater::{Error, Update};

extern "C" {
    fn annex_macos_install(
        bytes: *const u8,
        length: usize,
        signature: *const i8,
        version: *const i8,
    ) -> i32;
}

pub(crate) fn install(update: &Update, bytes: &[u8]) -> Result<(), Error> {
    update.verify(bytes)?;
    let string = |value: &str| {
        CString::new(value)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid update metadata"))
    };
    let signature = string(&update.signature)?;
    let version = string(&update.version)?;
    let result = unsafe {
        annex_macos_install(
            bytes.as_ptr(),
            bytes.len(),
            signature.as_ptr(),
            version.as_ptr(),
        )
    };
    match result {
        0 => Ok(()),
        3 => Err(io::Error::new(io::ErrorKind::TimedOut, "Installation service timed out").into()),
        _ => Err(io::Error::other("Installation service refused replacement").into()),
    }
}
