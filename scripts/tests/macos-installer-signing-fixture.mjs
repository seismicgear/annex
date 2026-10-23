// Disposable native component-test authority. Never used by release builds.
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import fs from 'node:fs';

const [mode, keyFile, archive, signatureFile] = process.argv.slice(2);
if (mode === 'create') {
  const { privateKey } = generateKeyPairSync('ed25519');
  fs.writeFileSync(keyFile, JSON.stringify({
    privateKey: privateKey.export({ type: 'pkcs8', format: 'pem' }),
    keyID: randomBytes(8).toString('hex'),
  }), { mode: 0o600, flag: 'wx' });
}
const fixture = JSON.parse(fs.readFileSync(keyFile, 'utf8'));
const privateKey = createPrivateKey(fixture.privateKey);
const keyID = Buffer.from(fixture.keyID, 'hex');
if (mode === 'create') {
  const raw = createPublicKey(privateKey).export({ type: 'spki', format: 'der' }).subarray(-32);
  console.log(Buffer.from(`untrusted comment: disposable native test key\n${Buffer.concat([Buffer.from('Ed'), keyID, raw]).toString('base64')}\n`).toString('base64'));
} else if (mode === 'sign') {
  const signature = sign(null, createHash('blake2b512').update(fs.readFileSync(archive)).digest(), privateKey);
  const comment = 'timestamp:1\tfile:native-component-test';
  const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), privateKey);
  fs.writeFileSync(signatureFile, Buffer.from(`untrusted comment: disposable native test signature\n${Buffer.concat([Buffer.from('ED'), keyID, signature]).toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64'));
} else {
  throw Error('Unsupported fixture operation');
}
