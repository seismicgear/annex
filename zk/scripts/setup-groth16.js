const fs = require("fs");
const path = require("path");
const { execSync } = require("child_process");

const buildPath = path.resolve(__dirname, "../build");
const keysPath = path.resolve(__dirname, "../keys");

if (!fs.existsSync(keysPath)) {
    fs.mkdirSync(keysPath);
}

const circuits = ['identity', 'membership'];

function run(cmd) {
    console.log(`Running: ${cmd}`);
    execSync(cmd, { stdio: 'inherit', cwd: path.resolve(__dirname, "..") });
}

async function setup() {
    const ptauPath = path.join(keysPath, "pot14_final.ptau");
    const ptau0 = path.join(keysPath, "pot14_0000.ptau");
    const ptau1 = path.join(keysPath, "pot14_0001.ptau");

    if (!fs.existsSync(ptauPath)) {
        console.log("Generating Powers of Tau...");
        // 1. Start a new powers of tau ceremony
        run(`npx snarkjs powersoftau new bn128 14 ${ptau0} -v`);
        // 2. Contribute to the ceremony
        run(`npx snarkjs powersoftau contribute ${ptau0} ${ptau1} --name="First Contribution" -v -e="random text"`);
        // 3. Prepare for phase 2
        run(`npx snarkjs powersoftau prepare phase2 ${ptau1} ${ptauPath} -v`);
    }

    for (const circuit of circuits) {
        console.log(`Setting up ${circuit}...`);
        const r1csPath = path.join(buildPath, `${circuit}.r1cs`);
        const zkey0 = path.join(keysPath, `${circuit}_0.zkey`);
        const zkeyFinal = path.join(keysPath, `${circuit}_final.zkey`);
        const vkeyPath = path.join(keysPath, `${circuit}_vkey.json`);

        // 4. Setup Phase 2
        run(`npx snarkjs groth16 setup ${r1csPath} ${ptauPath} ${zkey0}`);

        // 5. Contribute to Phase 2
        run(`npx snarkjs zkey contribute ${zkey0} ${zkeyFinal} --name="Second Contribution" -v -e="more entropy"`);

        // 6. Export verification key
        run(`npx snarkjs zkey export verificationkey ${zkeyFinal} ${vkeyPath}`);

        console.log(`${circuit} setup complete.`);
    }
}

setup().catch(err => {
    console.error(err);
    process.exit(1);
});
