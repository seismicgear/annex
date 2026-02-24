const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const isWindows = process.platform === 'win32';
const binName = isWindows ? 'circom.exe' : 'circom';
const binDir = path.resolve(__dirname, '../bin');
const binPath = path.join(binDir, binName);
const circuitsPath = path.resolve(__dirname, '../circuits');
const buildPath = path.resolve(__dirname, '../build');

// Auto-download circom if it's not present for the current platform.
if (!fs.existsSync(binPath)) {
    const version = 'v2.2.3';
    const platform = process.platform;
    const arch = process.arch;

    let assetName;
    if (platform === 'win32') {
        assetName = 'circom-windows-amd64.exe';
    } else if (platform === 'darwin') {
        assetName = arch === 'arm64' ? 'circom-macos-arm64' : 'circom-macos-amd64';
    } else {
        assetName = 'circom-linux-amd64';
    }

    const url = `https://github.com/iden3/circom/releases/download/${version}/${assetName}`;
    console.log(`circom not found at ${binPath}`);
    console.log(`Downloading circom ${version} for ${platform}-${arch}...`);

    if (!fs.existsSync(binDir)) {
        fs.mkdirSync(binDir, { recursive: true });
    }

    try {
        if (isWindows) {
            execSync(
                `powershell -Command "Invoke-WebRequest -Uri '${url}' -OutFile '${binPath}'"`,
                { stdio: 'inherit' }
            );
        } else {
            execSync(`curl -fL -o '${binPath}' '${url}'`, { stdio: 'inherit' });
            execSync(`chmod +x '${binPath}'`);
        }
        console.log('Download complete.');
    } catch (e) {
        console.error(`Failed to download circom from ${url}`);
        console.error('Please download it manually and place it at:', binPath);
        process.exit(1);
    }
}

if (!fs.existsSync(buildPath)) {
    fs.mkdirSync(buildPath);
}

const circuits = ['identity', 'membership'];

circuits.forEach(circuit => {
    console.log(`Building ${circuit}...`);
    const circuitPath = path.join(circuitsPath, `${circuit}.circom`);

    // Compile circuit
    // --r1cs: generate r1cs file
    // --wasm: generate wasm witness generator
    // --sym: generate symbols file
    // -o: output directory
    try {
        const cmd = `"${binPath}" "${circuitPath}" --r1cs --wasm --sym -o "${buildPath}" -l ./node_modules`;
        execSync(cmd, { stdio: 'inherit' });
        console.log(`Built ${circuit} successfully.`);
    } catch (e) {
        console.error(`Failed to build ${circuit}:`, e);
        process.exit(1);
    }
});
