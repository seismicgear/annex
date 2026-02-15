const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const binPath = path.resolve(__dirname, '../bin/circom');
const circuitsPath = path.resolve(__dirname, '../circuits');
const buildPath = path.resolve(__dirname, '../build');

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
        const cmd = `${binPath} ${circuitPath} --r1cs --wasm --sym -o ${buildPath} -l ./node_modules`;
        execSync(cmd, { stdio: 'inherit' });
        console.log(`Built ${circuit} successfully.`);
    } catch (e) {
        console.error(`Failed to build ${circuit}:`, e);
        process.exit(1);
    }
});
