const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const isWindows = process.platform === 'win32';
const binName = isWindows ? 'circom.exe' : 'circom';
const binDir = path.resolve(__dirname, '../bin');
const binPath = path.join(binDir, binName);
const circuitsPath = path.resolve(__dirname, '../circuits');
const buildPath = path.resolve(__dirname, '../build');

const CIRCOM_VERSION = 'v2.2.3';

// Does this circom binary actually RUN on the current host? A committed binary
// can exist yet be the wrong platform/arch — e.g. the tracked linux-amd64 ELF
// on a macOS arm64 runner — which fails with exit 126 "cannot execute binary
// file" rather than being "missing". A plain existence check (the old logic)
// therefore skipped the download on macOS and then tried to exec the Linux
// binary. Probe with `--version` instead.
function canExecute(bin) {
    if (!fs.existsSync(bin)) return false;
    try {
        execSync(`"${bin}" --version`, { stdio: 'ignore' });
        return true;
    } catch {
        return false;
    }
}

// iden3/circom publishes ONLY amd64 assets (no circom-macos-arm64 /
// circom-linux-arm64 — both 404). The macos-amd64 build runs on Apple Silicon
// via Rosetta 2 (present on GitHub's macOS runners), so darwin always uses the
// amd64 asset.
function circomAssetName() {
    if (isWindows) return 'circom-windows-amd64.exe';
    if (process.platform === 'darwin') return 'circom-macos-amd64';
    return 'circom-linux-amd64';
}

function downloadCircom(targetPath) {
    const asset = circomAssetName();
    const url = `https://github.com/iden3/circom/releases/download/${CIRCOM_VERSION}/${asset}`;
    console.log(`Downloading circom ${CIRCOM_VERSION} (${asset}) for ${process.platform}-${process.arch}...`);
    fs.mkdirSync(path.dirname(targetPath), { recursive: true });
    if (isWindows) {
        execSync(
            `powershell -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -UseBasicParsing -Uri '${url}' -OutFile '${targetPath}'"`,
            { stdio: 'inherit' }
        );
    } else {
        execSync(`curl -fL -o '${targetPath}' '${url}'`, { stdio: 'inherit' });
        execSync(`chmod +x '${targetPath}'`);
    }
}

// Resolve a circom binary that actually runs here:
//   1. the committed/cached binary at binPath, if it executes (Linux fast path);
//   2. otherwise a platform-tagged download — kept at a distinct path so it
//      never overwrites the tracked linux-amd64 binary and a wrong-arch
//      committed binary is bypassed rather than executed.
function resolveCircom() {
    if (canExecute(binPath)) return binPath;

    const taggedPath = path.join(
        binDir,
        `circom-${process.platform}-${process.arch}${isWindows ? '.exe' : ''}`
    );
    if (!canExecute(taggedPath)) {
        try {
            downloadCircom(taggedPath);
        } catch (e) {
            console.error(`Failed to download circom from the ${CIRCOM_VERSION} release.`);
            console.error('Place a runnable circom binary at:', binPath);
            console.error(e.message || e);
            process.exit(1);
        }
    }
    if (!canExecute(taggedPath)) {
        console.error(`circom at ${taggedPath} still does not execute on ${process.platform}-${process.arch}.`);
        if (process.platform === 'darwin' && process.arch === 'arm64') {
            console.error('Apple Silicon runs the macos-amd64 build via Rosetta 2: softwareupdate --install-rosetta --agree-to-license');
        }
        process.exit(1);
    }
    return taggedPath;
}

const circomBin = resolveCircom();

if (!fs.existsSync(buildPath)) {
    fs.mkdirSync(buildPath);
}

const circuits = ['identity', 'membership', 'membership_v2'];

circuits.forEach(circuit => {
    console.log(`Building ${circuit}...`);
    const circuitPath = path.join(circuitsPath, `${circuit}.circom`);

    // Compile circuit
    // --r1cs: generate r1cs file
    // --wasm: generate wasm witness generator
    // --sym: generate symbols file
    // -o: output directory
    try {
        const cmd = `"${circomBin}" "${circuitPath}" --r1cs --wasm --sym -o "${buildPath}" -l ./node_modules`;
        execSync(cmd, { stdio: 'inherit' });
        console.log(`Built ${circuit} successfully.`);
    } catch (e) {
        console.error(`Failed to build ${circuit}:`, e);
        process.exit(1);
    }
});
