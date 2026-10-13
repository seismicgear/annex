/** Build-time consistency gate; remote verifiers never establish client trust. */
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
const root = new URL('../', import.meta.url);
const read = path => readFile(new URL(path, root));
const manifest = JSON.parse(await read('zk/artifacts/membership_v2/manifest.json'));
const verifier = await read('zk/artifacts/membership_v2/membership_v2_vkey.json');
if (createHash('sha256').update(verifier).digest('hex') !== manifest.vkey_sha256) throw new Error('Pinned membership verifier artifact hash differs from the ceremony manifest.');
const source = (await read('client/src/lib/membership-v2-verifier.ts')).toString('utf8');
const declaration = /const membershipV2Verifier\s*=\s*([\s\S]*?)\s+as const;/.exec(source);
if (!declaration || !isDeepStrictEqual(JSON.parse(declaration[1]), JSON.parse(verifier))) throw new Error('The independently shipped client verifier differs from the pinned ceremony.');
for (const [path, expected] of [['membership_v2.wasm', manifest.wasm_sha256], ['membership_v2_final.zkey', manifest.zkey_sha256]]) {
  if (createHash('sha256').update(await read(`client/public/zk/${path}`)).digest('hex') !== expected) {
    throw new Error(`Client proof artifact ${path} does not match the independently pinned verifier. Install the pinned ceremony artifacts before building.`);
  }
}
console.log('Client verifier and proof artifacts match the pinned membership-v2 ceremony.');
