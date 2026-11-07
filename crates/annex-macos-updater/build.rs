fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_SIGNING_PUBLIC_KEY");
    let key = std::env::var("TAURI_SIGNING_PUBLIC_KEY").unwrap_or_default();
    // A public verification key, never the private release authority. Builds
    // without update configuration retain a service that refuses installation.
    assert!(
        !key.contains(['\n', '\r']),
        "Invalid updater public key encoding"
    );
    println!("cargo:rustc-env=ANNEX_INSTALLER_PUBLIC_KEY={key}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/service.m");
        cc::Build::new()
            .file("src/service.m")
            .flag("-fblocks")
            .flag("-mmacosx-version-min=11.0")
            .compile("annex_update_service");
        for framework in ["Foundation", "Security", "AppKit"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
