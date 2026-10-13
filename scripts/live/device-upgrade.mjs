// Two-process upgrade acceptance. Prepare against the captured pre-certificate
// server, restart that SAME database with the new candidate, then verify.
// The disposable identity/device secret file stays in the private session dir.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {generateKeyPairSync,createPrivateKey,createPublicKey,diffieHellman,hkdfSync,randomBytes,createCipheriv,createDecipheriv} from 'node:crypto';
import {register,authenticate,request,digest,createChannel,joinChannel,socket,history} from './protocol.mjs';
import {makeDeviceCertificate,certificateDigest} from './device-certificates.mjs';

const [mode,recordPath]=process.argv.slice(2);
assert(['prepare','verify'].includes(mode) && recordPath,'prepare|verify <isolated session JSON>');
const session=JSON.parse(readFileSync(recordPath,'utf8').replace(/^\uFEFF/,''));
const privatePath=join(session.data,'device-upgrade-private.json');
const evidencePath=join(session.data,'device-upgrade-evidence.json');
const publicKey=key=>createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32);
const importPublic=raw=>createPublicKey({format:'der',type:'spki',key:Buffer.concat([Buffer.from('302a300506032b656e032100','hex'),raw])});
function encrypt(key,plaintext,aad) {
  const nonce=randomBytes(12),cipher=createCipheriv('chacha20-poly1305',key,nonce,{authTagLength:16});
  cipher.setAAD(aad,{plaintextLength:plaintext.length});
  const ciphertext=Buffer.concat([cipher.update(plaintext),cipher.final()]);
  return Buffer.concat([nonce,ciphertext,cipher.getAuthTag()]);
}
function decrypt(key,blob,aad) {
  const decipher=createDecipheriv('chacha20-poly1305',key,blob.subarray(0,12),{authTagLength:16});
  decipher.setAAD(aad,{plaintextLength:blob.length-28});decipher.setAuthTag(blob.subarray(-16));
  return Buffer.concat([decipher.update(blob.subarray(12,-16)),decipher.final()]);
}
function seal(plaintext,recipient) {
  const ephemeral=generateKeyPairSync('x25519').privateKey,epk=publicKey(ephemeral);
  const shared=diffieHellman({privateKey:ephemeral,publicKey:importPublic(recipient)});
  const key=hkdfSync('sha256',shared,Buffer.concat([epk,recipient]),'annex-e2e-seal-v1',32);
  return Buffer.concat([epk,encrypt(key,plaintext,epk)]);
}
function open(blob,secret) {
  const epk=blob.subarray(0,32),recipient=publicKey(secret);
  const shared=diffieHellman({privateKey:secret,publicKey:importPublic(epk)});
  return decrypt(hkdfSync('sha256',shared,Buffer.concat([epk,recipient]),'annex-e2e-seal-v1',32),blob.subarray(32),epk);
}
let connection;
const evidence=mode==='verify'?JSON.parse(readFileSync(evidencePath,'utf8')):{complete:false,receipts:[]};
function record(action,details={}) {evidence.receipts.push({at:new Date().toISOString(),action,...details});console.log(`[device-upgrade] ${action}`);}
try {
  if(mode==='prepare') {
    const identity=await register(session.url),secret=generateKeyPairSync('x25519').privateKey;
    const pub=publicKey(secret),deviceId=digest(pub),channel=`upgrade-${Date.now()}`;
    assert.equal((await request(session.url,'/api/keys/me',{method:'PUT',token:identity.token,body:{version:2,x25519_pub_hex:pub.toString('hex'),device_certificate:null}})).created,true);
    await createChannel(identity,channel,'Text','Local');await joinChannel(identity,channel);
    await request(session.url,`/api/channels/${channel}/e2e`,{method:'PUT',token:identity.token,body:{enabled:true}});
    const directory=await request(session.url,`/api/channels/${channel}/member-keys`,{token:identity.token});
    const cek=randomBytes(32),keyId=digest(Buffer.concat([Buffer.from('annex-channel-key-v2\0'),cek]));
    const epoch=directory.required_epoch,authority=directory.authority_server;
    const wrapFields=['annex-key-wrap-v2',authority,channel,epoch,keyId,identity.pseudonym,deviceId,cek.toString('base64')];
    const wrapped=seal(Buffer.from(JSON.stringify(wrapFields)),pub).toString('base64');
    await request(session.url,`/api/channels/${channel}/key-wraps`,{method:'POST',token:identity.token,body:{version:2,operation:'commit_epoch',expected_revision:directory.revision,epoch,key_id:keyId,sender_device_id:deviceId,wraps:[{recipient_pseudonym_id:identity.pseudonym,recipient_device_id:deviceId,wrapped_key_b64:wrapped}]}});
    const plaintext=`Encrypted migration acceptance ${randomBytes(16).toString('hex')}`;
    const aad=Buffer.from(JSON.stringify(['annex-message-v2',authority,channel,epoch,keyId]));
    const ciphertext=`e2e2:${epoch}:${keyId}:${encrypt(cek,Buffer.from(plaintext),aad).toString('base64')}`;
    connection=await socket(identity);connection.send({type:'subscribe',channelId:channel});
    connection.send({type:'message',channelId:channel,content:ciphertext,clientRequestId:'legacy-before-upgrade'});
    const message=await connection.wait(frame=>frame.type==='message'&&frame.content===ciphertext,'actual encrypted message before upgrade');
    writeFileSync(privatePath,JSON.stringify({identity:{url:identity.url,sk:String(identity.sk),nodeId:identity.nodeId,roleCode:identity.roleCode,topic:identity.topic,commitment:identity.commitment,pseudonym:identity.pseudonym},private_key_b64:secret.export({format:'der',type:'pkcs8'}).toString('base64'),channel,deviceId,authority,epoch,keyId,wrapped,ciphertext,messageId:message.messageId,plaintext_sha256:digest(plaintext)},null,2));
    evidence.before_binary_sha256=session.binary_sha256;
    record('Pre-certificate server accepted a real device, sealed key epoch and encrypted WebSocket message',{device_id:deviceId,channel,epoch,ciphertext_sha256:digest(ciphertext)});
  } else {
    const saved=JSON.parse(readFileSync(privatePath,'utf8'));
    assert.notEqual(session.binary_sha256,evidence.before_binary_sha256,'Upgrade must execute a different captured binary');
    const identity={...saved.identity,sk:BigInt(saved.identity.sk)};await authenticate(identity);
    const secret=createPrivateKey({key:Buffer.from(saved.private_key_b64,'base64'),format:'der',type:'pkcs8'});
    const before=await request(session.url,`/api/keys/${identity.pseudonym}`,{token:identity.token});
    assert.equal(before.devices.find(d=>d.device_id===saved.deviceId).device_certificate,null);
    const context=await request(session.url,'/api/keys/certificate-context',{token:identity.token});
    const {certificate}=await makeDeviceCertificate(identity,context,{device:{privateKey:secret}});
    const upgraded=await request(session.url,'/api/keys/me',{method:'PUT',token:identity.token,body:{version:2,x25519_pub_hex:certificate.x25519_pub_hex,device_certificate:certificate}});
    assert.equal(upgraded.created,false);assert.equal(upgraded.device_id,saved.deviceId);
    const page=await request(session.url,`/api/channels/${saved.channel}/key-wraps?device_id=${saved.deviceId}`,{token:identity.token});
    const row=page.wraps.find(w=>w.epoch===saved.epoch);assert(row,'Existing encrypted epoch remains readable');assert.equal(row.wrapped_key_b64,saved.wrapped);
    const fields=JSON.parse(open(Buffer.from(row.wrapped_key_b64,'base64'),secret).toString('utf8'));
    assert.deepEqual(fields.slice(0,7),['annex-key-wrap-v2',saved.authority,saved.channel,saved.epoch,saved.keyId,identity.pseudonym,saved.deviceId]);
    const cek=Buffer.from(fields[7],'base64');assert.equal(digest(Buffer.concat([Buffer.from('annex-channel-key-v2\0'),cek])),saved.keyId);
    const message=(await history(identity,saved.channel)).find(m=>m.message_id===saved.messageId);assert.equal(message.content,saved.ciphertext);
    const aad=Buffer.from(JSON.stringify(['annex-message-v2',saved.authority,saved.channel,saved.epoch,saved.keyId]));
    assert.equal(digest(decrypt(cek,Buffer.from(message.content.split(':')[3],'base64'),aad)),saved.plaintext_sha256);
    record('New binary migrated the real database; original device upgraded and original encrypted message still decrypts',{certificate_sha256:certificateDigest(certificate),after_binary_sha256:session.binary_sha256});
    evidence.complete=true;
  }
} catch(error) {record('FAILED',{error:error.message});process.exitCode=1;}
finally {connection?.ws.close();writeFileSync(evidencePath,JSON.stringify(evidence,null,2));}
process.exit(process.exitCode??0);
