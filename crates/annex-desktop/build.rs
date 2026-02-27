fn main() {
    // On Linux, verify that WebKitGTK is new enough for WebRTC support.
    // getDisplayMedia() and getUserMedia() require WebKitGTK >= 2.40.
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = std::process::Command::new("pkg-config")
            .args(["--modversion", "webkitgtk-4.1"])
            .output()
        {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let parts: Vec<u32> = version.split('.').filter_map(|s| s.parse().ok()).collect();
                if parts.len() >= 2 {
                    let (major, minor) = (parts[0], parts[1]);
                    if major < 2 || (major == 2 && minor < 40) {
                        println!(
                            "cargo:warning=WebKitGTK {version} detected. \
                             WebRTC (getUserMedia, getDisplayMedia) requires >= 2.40. \
                             Screen sharing and camera/mic access may not work."
                        );
                    }
                }
            }
        }
    }

    tauri_build::build();
}
