use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use tempfile::TempDir;

static ZK_SETUP: Once = Once::new();

pub fn get_project_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn ensure_zk_artifacts(root: &Path) {
    ZK_SETUP.call_once(|| {
        let zk_dir = root.join("zk");
        let build_dir = zk_dir.join("build");
        let keys_dir = zk_dir.join("keys");

        // Check for essential artifacts for identity and membership
        // This list should match what the tests use.
        // identity_js/identity.wasm, identity_final.zkey, identity_vkey.json
        // membership_js/membership.wasm, membership_final.zkey, membership_vkey.json
        // If any missing, rebuild.
        // Actually, setup-groth16.js checks if ptau exists, but rebuilds keys if scripts run.
        // But build-circuits.js might overwrite.

        let identity_wasm = build_dir.join("identity_js/identity.wasm");
        let identity_zkey = keys_dir.join("identity_final.zkey");
        let identity_vkey = keys_dir.join("identity_vkey.json");

        if identity_wasm.exists() && identity_zkey.exists() && identity_vkey.exists() {
            // Assume other artifacts exist too if identity exists.
            return;
        }

        println!("ZK artifacts missing. Building circuits and performing setup...");

        // Ensure bin/circom is executable (if checked out freshly)
        let circom_bin = zk_dir.join("bin/circom");
        if circom_bin.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&circom_bin).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&circom_bin, perms).unwrap();
            }
        }

        // npm install
        let status = Command::new("npm")
            .current_dir(&zk_dir)
            .arg("install")
            .status()
            .expect("failed to run npm install");
        assert!(status.success(), "npm install failed");

        // build-circuits.js
        let status = Command::new("node")
            .current_dir(&zk_dir)
            .arg("scripts/build-circuits.js")
            .status()
            .expect("failed to run build-circuits.js");
        assert!(status.success(), "build-circuits.js failed");

        // setup-groth16.js
        let status = Command::new("node")
            .current_dir(&zk_dir)
            .arg("scripts/setup-groth16.js")
            .status()
            .expect("failed to run setup-groth16.js");
        assert!(status.success(), "setup-groth16.js failed");
    });
}

pub struct ZkPaths {
    pub wasm: PathBuf,
    pub zkey: PathBuf,
    pub witness_gen: PathBuf,
}

pub fn get_zk_paths(circuit_name: &str) -> ZkPaths {
    let root = get_project_root();
    ensure_zk_artifacts(&root);

    let zk_build = root.join("zk/build");
    let zk_keys = root.join("zk/keys");

    ZkPaths {
        wasm: zk_build.join(format!("{}_js/{}.wasm", circuit_name, circuit_name)),
        witness_gen: zk_build.join(format!("{}_js/generate_witness.js", circuit_name)),
        zkey: zk_keys.join(format!("{}_final.zkey", circuit_name)),
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
    ensure_zk_artifacts(&root);

    let key_path = root.join(format!("zk/keys/{}_vkey.json", circuit_name));
    fs::read_to_string(key_path).expect("failed to read verification key")
}
