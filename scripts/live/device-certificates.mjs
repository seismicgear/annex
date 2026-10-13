// Actual certificate acceptance. Every identity and proof uses the live registry;
// neither session tokens nor membership rows are fabricated. Receipt is public
// metadata only. Private proving/device material remains in this process.
import assert from 'node:assert/strict';
import { createPublicKey, generateKeyPairSync, randomBytes } from 'node:crypto';
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { register, request, digest, hex, signingIdentityFromSeed, createChannel, joinChannel } from './protocol.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const requireZk = createRequire(new URL('../../zk/package.json', import.meta.url));
const { groth16 } = requireZk('snarkjs');
const prime = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
export const certificateBody = c => JSON.stringify(['annex-device-certificate-v1', c.server_identity, c.topic, c.pseudonym_id, c.device_id, c.x25519_pub_hex, c.ed25519_pub_hex, c.root_hex, c.commitment_hex, c.nullifier_hex, c.generation, c.previous_certificate_sha256]);
export const certificateDigest = c => digest(certificateBody(c));
export const certificateChallenge = c => hex(BigInt(`0x${certificateDigest(c)}`) % prime);
export function verifierFingerprint(vkey) {
  const g1 = point => point.slice(0,2).map(value => BigInt(value).toString());
  const g2 = point => point.slice(0,2).map(row => row.slice(0,2).map(value => BigInt(value).toString()));
  assert.equal(vkey.IC.length,6);
  return digest(JSON.stringify(['annex-membership-v2-verifier-v1',g1(vkey.vk_alpha_1),g2(vkey.vk_beta_2),g2(vkey.vk_gamma_2),g2(vkey.vk_delta_2),vkey.IC.map(g1)]));
}
export async function makeDeviceCertificate(identity, context, { previous = null, device = null, signingSeed = null } = {}) {
  const key = device ?? generateKeyPairSync('x25519');
  const publicHex = createPublicKey(key.privateKey).export({format:'der',type:'spki'}).subarray(-32).toString('hex');
  const signing = signingIdentityFromSeed(signingSeed ?? randomBytes(32).toString('hex'));
  const path = await request(identity.url, `/api/registry/path/${identity.commitment}`);
  const certificate = {
    version:1, server_identity:context.server_identity, topic:identity.topic, pseudonym_id:identity.pseudonym,
    device_id:digest(Buffer.from(publicHex,'hex')), x25519_pub_hex:publicHex, ed25519_pub_hex:signing.publicKey,
    root_hex:path.rootHex, commitment_hex:identity.commitment, nullifier_hex:context.nullifier_hex,
    generation:previous ? previous.generation+1 : 1,
    previous_certificate_sha256:previous ? certificateDigest(previous) : null,
    proof:{}, public_signals:[], signature_hex:'',
  };
  const challenge = certificateChallenge(certificate);
  const topicHash = BigInt(`0x${digest(`annex/v2/topicHash:${identity.topic}`)}`) % prime;
  const {proof,publicSignals} = await groth16.fullProve({
    sk:String(identity.sk),roleCode:String(identity.roleCode),nodeId:String(identity.nodeId),
    leafIndex:String(path.leafIndex),pathElements:path.pathElements.map(value=>`0x${value}`),
    pathIndexBits:path.pathIndexBits.map(String),topicHash:String(topicHash),challenge:String(BigInt(`0x${challenge}`)),
  }, `${root}zk/build/membership_v2_js/membership_v2.wasm`, `${root}zk/keys/membership_v2_final.zkey`);
  certificate.proof=proof; certificate.public_signals=publicSignals.map(hex);
  assert.deepEqual(certificate.public_signals,[certificate.root_hex,certificate.commitment_hex,certificate.nullifier_hex,hex(topicHash),challenge]);
  certificate.signature_hex=signing.sign(JSON.stringify(['annex-device-possession-v1',certificateDigest(certificate)]));
  const vkey=JSON.parse(readFileSync(`${root}zk/keys/membership_v2_vkey.json`,'utf8'));
  assert.equal(verifierFingerprint(vkey),context.verifier_sha256);
  assert(await groth16.verify(vkey,publicSignals,proof), 'Independent actual Groth16 certificate verification');
  return {certificate,signing,device:key};
}

async function main(session) {
  const receipts=[];
  const record=(action,details={})=>{receipts.push({at:new Date().toISOString(),action,...details}); console.log(`[device-certificate-live] ${action}`);};
  let complete=false;
  try {
    const founder=await register(session.url);
    const context=await request(session.url,'/api/keys/certificate-context',{token:founder.token});
    assert.equal(context.server_identity,`annex-server:${digest(Buffer.from(context.server_ed25519_pub_hex,'hex'))}`);
    assert.equal(context.commitment_hex,founder.commitment); assert.equal(context.topic,founder.topic);
    const {certificate,device}=await makeDeviceCertificate(founder,context);
    const channel=`certificate-${Date.now()}`;
    await createChannel(founder,channel,'Text','Local'); await joinChannel(founder,channel);
    const publish=(cert,token=founder.token,expected=200)=>request(session.url,'/api/keys/me',{
      method:'PUT',token,expected,body:{version:2,x25519_pub_hex:cert.x25519_pub_hex,device_certificate:cert},
    });
    assert.equal((await publish(certificate)).created,true);
    assert.equal((await publish(certificate)).created,false);
    await request(session.url,'/api/keys/me',{method:'PUT',token:founder.token,expected:400,
      body:{version:2,x25519_pub_hex:certificate.x25519_pub_hex,device_certificate:null}});
    await publish({...certificate,unrecognized_authorization:true},founder.token,400);
    await request(session.url,'/api/keys/me',{method:'PUT',token:founder.token,expected:400,
      body:{version:1,x25519_pub_hex:certificate.x25519_pub_hex,device_certificate:certificate}});
    const directory=await request(session.url,`/api/keys/${founder.pseudonym}`,{token:founder.token});
    assert.deepEqual(directory.devices.find(device=>device.device_id===certificate.device_id).device_certificate,certificate);
    record('Real certificate accepted, independently verified, read back unchanged and idempotently republished; null, downgraded and unrecognized authorization refused',{certificate_sha256:certificateDigest(certificate)});
    for(const field of ['server_identity','pseudonym_id','ed25519_pub_hex','root_hex','nullifier_hex']) {
      const tampered=structuredClone(certificate);
      tampered[field]=field==='server_identity'?`annex-server:${'0'.repeat(64)}`:'0'.repeat(64);
      await publish(tampered,founder.token,400);
    }
    const badProof=structuredClone(certificate); badProof.proof.pi_a[0]='1'; await publish(badProof,founder.token,400);
    const badSignature=structuredClone(certificate); badSignature.signature_hex='0'.repeat(128); await publish(badSignature,founder.token,400);
    const attacker=await register(session.url);
    await publish(certificate,attacker.token,400);
    record('Substituted keys, context, proof, signature and authenticated identity all refused');
    await request(session.url,'/api/zk/verify-membership',{method:'POST',expected:401,body:{
      root:certificate.root_hex,commitment:certificate.commitment_hex,topic:certificate.topic,
      proof:certificate.proof,publicSignals:certificate.public_signals.map(value=>String(BigInt(`0x${value}`))),
      protocolVersion:'v2',nullifierHex:certificate.nullifier_hex,topicHashHex:certificate.public_signals[3],challengeHex:certificate.public_signals[4],
    }});
    record('Static device certificate proof cannot authenticate a session');
    const replaced=await makeDeviceCertificate(founder,context,{previous:certificate});
    assert.equal((await publish(replaced.certificate)).created,true);
    const conflict=await makeDeviceCertificate(founder,context,{device});
    await publish(conflict.certificate,founder.token,409);
    record('A fresh valid proof cannot overwrite an already certified device with another signing key');
    await request(session.url,`/api/keys/me/${certificate.device_id}`,{method:'DELETE',token:founder.token});
    await publish(certificate,founder.token,403);
    record('Explicit replacement lineage admitted; revoked certified material cannot be revived');
    // Only public material is retained for Rust independent-verifier regression.
    writeFileSync(join(session.data,'device-certificate-public-fixture.json'),JSON.stringify({certificate,vkey:JSON.parse(readFileSync(`${root}zk/keys/membership_v2_vkey.json`,'utf8')),verifier_sha256:context.verifier_sha256},null,2));
    complete=true;
  } catch(error) { record('FAILED',{error:error.message}); process.exitCode=1; }
  finally { writeFileSync(join(session.data,'device-certificate-live-evidence.json'),JSON.stringify({complete,binary_sha256:session.binary_sha256,receipts},null,2)); }
  return complete;
}
if(process.argv[1]===fileURLToPath(import.meta.url)) {
  const session=JSON.parse(readFileSync(process.argv[2],'utf8').replace(/^\uFEFF/,''));
  process.exit(await main(session)?0:1);
}
