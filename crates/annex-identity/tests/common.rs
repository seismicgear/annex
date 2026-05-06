use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, Once};
use tempfile::TempDir;

static ZK_SETUP: Once = Once::new();
/// Outcome of `ensure_zk_artifacts`. `Ok(())` means artifacts are present
/// (either already on disk, or freshly built); `Err(reason)` means the
/// caller should `return` early — there's no usable ZK toolchain.
static ZK_OUTCOME: Mutex<Option<Result<(), String>>> = Mutex::new(None);

pub fn get_project_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Ensures ZK artifacts are present, building them on demand. Returns
/// `Ok(())` on success or `Err(reason)` when artifacts cannot be built
/// (typically because the sandbox lacks a working circom toolchain).
/// Tests should check the result and gracefully skip when it errs — CI
/// runs `node zk/scripts/dev-setup-groth16.js` up-front so the real
/// path always exercises the full proof flow.
pub fn ensure_zk_artifacts(root: &Path) -> Result<(), String> {
    ZK_SETUP.call_once(|| {
        let outcome = build_zk_artifacts(root);
        *ZK_OUTCOME.lock().unwrap() = Some(outcome);
    });
    ZK_OUTCOME
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| Err("ensure_zk_artifacts not initialised".to_string()))
}

fn build_zk_artifacts(root: &Path) -> Result<(), String> {
    let zk_dir = root.join("zk");
    let build_dir = zk_dir.join("build");
    let keys_dir = zk_dir.join("keys");

    // Check for essential artifacts for identity and membership.
    let identity_wasm = build_dir.join("identity_js/identity.wasm");
    let identity_zkey = keys_dir.join("identity_final.zkey");
    let identity_vkey = keys_dir.join("identity_vkey.json");

    if identity_wasm.exists() && identity_zkey.exists() && identity_vkey.exists() {
        // Assume other artifacts exist too if identity exists.
        return Ok(());
    }

    eprintln!(
        "[annex-identity tests] ZK artifacts missing. Building circuits and performing setup..."
    );

    // Ensure bin/circom is executable (if checked out freshly)
    let circom_bin = zk_dir.join("bin/circom");
    if circom_bin.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = fs::metadata(&circom_bin).map(|m| m.permissions()) {
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&circom_bin, perms);
            }
        }
    }

    // npm install
    let status = Command::new("npm")
        .current_dir(&zk_dir)
        .arg("install")
        .status()
        .map_err(|e| format!("failed to spawn npm: {e}"))?;
    if !status.success() {
        return Err(format!("npm install failed (exit {status})"));
    }

    // build-circuits.js
    let status = Command::new("node")
        .current_dir(&zk_dir)
        .arg("scripts/build-circuits.js")
        .status()
        .map_err(|e| format!("failed to spawn node for build-circuits.js: {e}"))?;
    if !status.success() {
        return Err(format!(
            "build-circuits.js failed (exit {status}) — \
             likely a sandbox without a working circom toolchain. \
             Tests requiring real ZK artifacts will skip."
        ));
    }

    // setup-groth16.js
    let status = Command::new("node")
        .current_dir(&zk_dir)
        .arg("scripts/setup-groth16.js")
        .status()
        .map_err(|e| format!("failed to spawn node for setup-groth16.js: {e}"))?;
    if !status.success() {
        return Err(format!("setup-groth16.js failed (exit {status})"));
    }

    Ok(())
}

pub struct ZkPaths {
    pub wasm: PathBuf,
    pub zkey: PathBuf,
    pub witness_gen: PathBuf,
}

pub fn get_zk_paths(circuit_name: &str) -> ZkPaths {
    let root = get_project_root();
    if let Err(e) = ensure_zk_artifacts(&root) {
        panic!("ZK artifacts unavailable: {e}");
    }

    let zk_build = root.join("zk/build");
    let zk_keys = root.join("zk/keys");

    ZkPaths {
        wasm: zk_build.join(format!("{circuit_name}_js/{circuit_name}.wasm")),
        witness_gen: zk_build.join(format!("{circuit_name}_js/generate_witness.js")),
        zkey: zk_keys.join(format!("{circuit_name}_final.zkey")),
    }
}

/// Returns true if the ZK toolchain is available (artifacts on disk and/or
/// buildable in this environment). Tests that call `generate_proof` /
/// `get_verification_key` should call this first and `return` on `false`.
/// This exists because circom does not always compile in the test sandbox;
/// CI runs `node zk/scripts/dev-setup-groth16.js` before tests, so the real
/// path always exercises the full proof flow.
pub fn zk_toolchain_available() -> bool {
    let root = get_project_root();
    match ensure_zk_artifacts(&root) {
        Ok(()) => true,
        Err(reason) => {
            eprintln!("[annex-identity tests] skipping ZK-dependent test: {reason}");
            false
        }
    }
}

pub fn generate_proof(
    circuit_name: &str,
    input_json: &serde_json::Value,
) -> (serde_json::Value, serde_json::Value) {
    let paths = get_zk_paths(circuit_name); // This ensures artifacts exist
    let temp_dir = TempDir::new().unwrap();
    let input_path = temp_dir.path().join("input.json");
    let witness_path = temp_dir.path().join("witness.wtns");
    let proof_path = temp_dir.path().join("proof.json");
    let public_path = temp_dir.path().join("public.json");

    // Write input JSON
    let input_str = serde_json::to_string(input_json).unwrap();
    fs::write(&input_path, input_str).unwrap();

    // Generate witness
    let status = Command::new("node")
        .arg(&paths.witness_gen)
        .arg(&paths.wasm)
        .arg(&input_path)
        .arg(&witness_path)
        .status()
        .expect("failed to run generate_witness.js");
    assert!(status.success(), "generate_witness.js failed");

    // Generate proof
    // Use npx in the zk directory to ensure we use the project's snarkjs version
    let root = get_project_root();
    let zk_dir = root.join("zk");

    let status = Command::new("npx")
        .current_dir(&zk_dir)
        .arg("snarkjs")
        .arg("groth16")
        .arg("prove")
        .arg(&paths.zkey)
        .arg(&witness_path)
        .arg(&proof_path)
        .arg(&public_path)
        .status()
        .expect("failed to run snarkjs");
    assert!(status.success(), "snarkjs prove failed");

    // Read proof and public signals
    let proof_str = fs::read_to_string(&proof_path).unwrap();
    let public_str = fs::read_to_string(&public_path).unwrap();

    let proof: serde_json::Value = serde_json::from_str(&proof_str).unwrap();
    let public: serde_json::Value = serde_json::from_str(&public_str).unwrap();

    (proof, public)
}

pub fn get_verification_key(circuit_name: &str) -> String {
    let root = get_project_root();
    if let Err(e) = ensure_zk_artifacts(&root) {
        panic!("ZK artifacts unavailable: {e}");
    }

    let key_path = root.join(format!("zk/keys/{circuit_name}_vkey.json"));
    fs::read_to_string(key_path).expect("failed to read verification key")
}
