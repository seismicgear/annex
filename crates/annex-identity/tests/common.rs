use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, Once};
use tempfile::TempDir;

static ZK_SETUP: Once = Once::new();
/// Outcome of `ensure_zk_artifacts`. `Ok(())` means artifacts are present
/// and match the pinned ceremony; failures must fail the proof test.
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

/// Verifies and installs the pinned ceremony. Missing dependencies fail the test.
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
    if !zk_dir.join("node_modules/snarkjs/build/cli.cjs").is_file() {
        return Err("snarkjs is missing; run npm ci in zk before the proof tests".into());
    }
    // Test the shipped ceremony, including its installed verification keys.
    // Never silently replace release artifacts with a fresh development setup.
    let status = Command::new("node")
        .current_dir(root)
        .arg("zk/scripts/verify-artifacts.js")
        .args(["--all", "--profile", "production"])
        .status()
        .map_err(|e| format!("could not verify pinned ZK artifacts: {e}"))?;
    if !status.success() {
        return Err(format!("pinned ZK verification failed: {status}"));
    }
    let status = Command::new("node")
        .current_dir(root)
        .arg("zk/scripts/install-ceremony.js")
        .status()
        .map_err(|e| format!("could not install pinned ZK artifacts: {e}"))?;
    if !status.success() {
        return Err(format!("pinned ZK installation failed: {status}"));
    }
    Ok(())
}
pub struct ZkPaths {
    pub wasm: PathBuf,
    pub zkey: PathBuf,
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
        zkey: zk_keys.join(format!("{circuit_name}_final.zkey")),
    }
}

pub fn generate_proof(
    circuit_name: &str,
    input_json: &serde_json::Value,
) -> (serde_json::Value, serde_json::Value) {
    let paths = get_zk_paths(circuit_name); // This ensures artifacts exist
    let temp_dir = TempDir::new().unwrap();
    let input_path = temp_dir.path().join("input.json");
    let proof_path = temp_dir.path().join("proof.json");
    let public_path = temp_dir.path().join("public.json");

    // Write input JSON
    let input_str = serde_json::to_string(input_json).unwrap();
    fs::write(&input_path, input_str).unwrap();

    // Pinned ceremony artifacts contain WASM and keys, not circom's generated
    // JavaScript wrappers. Use the installed snarkjs entry point directly so
    // the exact pinned artifacts work on Windows as well as Unix.
    let root = get_project_root();
    let zk_dir = root.join("zk");

    let status = Command::new("node")
        .current_dir(&zk_dir)
        .arg(zk_dir.join("node_modules/snarkjs/build/cli.cjs"))
        .arg("groth16")
        .arg("fullprove")
        .arg(&input_path)
        .arg(&paths.wasm)
        .arg(&paths.zkey)
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
