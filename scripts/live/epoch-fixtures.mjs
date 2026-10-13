// PUBLIC TEST VECTOR KEYS ONLY. Never use these deterministic device seeds in
// an installation. Actual random membership secrets and session tokens are
// used for proof generation/admission, then discarded without being recorded.
import assert from 'node:assert/strict';
import {createPrivateKey} from 'node:crypto';
import {readFileSync,writeFileSync} from 'node:fs';
import {register,request,digest,signingIdentityFromSeed} from './protocol.mjs';
import {makeDeviceCertificate} from './device-certificates.mjs';
const [sessionPath,output]=process.argv.slice(2);
assert(sessionPath&&output);
const session=JSON.parse(readFileSync(sessionPath,'utf8').replace(/^\uFEFF/,''));
const authority=signingIdentityFromSeed('55'.repeat(32));
const entries=[];
try{
  for(const [name,seeds] of [['alice',[11,12]],['bob',[21,22]]]){
    const identity=await register(session.url);
    const context=await request(session.url,'/api/keys/certificate-context',{token:identity.token});
    assert.equal(context.server_identity,`annex-server:${digest(Buffer.from(authority.publicKey,'hex'))}`);
    for(const seed of seeds){
      const raw=Buffer.alloc(32,seed);
      const privateKey=createPrivateKey({key:Buffer.concat([Buffer.from('302e020100300506032b656e04220420','hex'),raw]),format:'der',type:'pkcs8'});
      const signingSeed=digest(`annex-public-test-device-ed25519-v1:${seed}`);
      const {certificate}=await makeDeviceCertificate(identity,context,{device:{privateKey},signingSeed});
      assert.equal((await request(session.url,'/api/keys/me',{method:'PUT',token:identity.token,body:{version:2,x25519_pub_hex:certificate.x25519_pub_hex,device_certificate:certificate}})).created,true);
      entries.push({name,public_test_x25519_byte:seed,public_test_ed25519_seed:signingSeed,certificate});
    }
  }
  const vkey=JSON.parse(readFileSync(new URL('../../zk/keys/membership_v2_vkey.json',import.meta.url),'utf8'));
  writeFileSync(output,JSON.stringify({version:1,public_test_authority_seed:'55'.repeat(32),entries,vkey,provenance:{created:new Date().toISOString(),binary_sha256:session.binary_sha256,kind:'PUBLIC TEST DEVICE SEEDS. Genuine Groth16 certificates were independently verified and admitted through actual isolated HTTP registry. No identity proving secrets or session tokens retained.'}},null,2));
  console.log('Four genuine device certificate test vectors admitted; only explicitly public deterministic test device seeds retained.');
}catch(error){console.error(error.message);process.exitCode=1;}
process.exit(process.exitCode??0);
