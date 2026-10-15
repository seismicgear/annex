// Real isolated HTTP/WebSocket acceptance. Reads disposable test-only secrets
// outside the repository; receipts never contain private keys or channel keys.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {createHash,createPrivateKey,createPublicKey,generateKeyPairSync,diffieHellman,hkdfSync,randomBytes,sign,verify,createCipheriv,createDecipheriv} from 'node:crypto';
import {authenticate,request as proofRequest} from './protocol.mjs';

const [sessionPath,privatePath]=process.argv.slice(2);
assert(sessionPath&&privatePath,'manifest.mjs <isolated session JSON> <task-work private fixture>');
const session=JSON.parse(readFileSync(sessionPath,'utf8').replace(/^\uFEFF/,''));
const fixture=JSON.parse(readFileSync(privatePath,'utf8'));
const evidence={complete:false,binary_sha256:session.binary_sha256,receipts:[]};
const sockets=[];
const hash=value=>createHash('sha256').update(value).digest('hex');
const canonical=value=>Array.isArray(value)?value.map(canonical):value&&typeof value==='object'?Object.fromEntries(Object.keys(value).sort().map(k=>[k,canonical(value[k])])):value;
const text=value=>JSON.stringify(canonical(value));
const privateEd=seed=>createPrivateKey({key:Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),Buffer.from(seed,'hex')]),format:'der',type:'pkcs8'});
const publicRaw=key=>createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32);
const xPublic=raw=>createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b656e032100','hex'),raw]),format:'der',type:'spki'});
const members=fixture.members.map(m=>({...m,identity:{...m.identity,sk:BigInt(m.identity.sk)},signing:privateEd(m.ed25519_seed_hex),secret:createPrivateKey({key:Buffer.from(m.x25519_pkcs8_base64,'base64'),format:'der',type:'pkcs8'})}));
function certificateHash(c){return hash(JSON.stringify(['annex-device-certificate-v1',c.server_identity,c.topic,c.pseudonym_id,c.device_id,c.x25519_pub_hex,c.ed25519_pub_hex,c.root_hex,c.commitment_hex,c.nullifier_hex,c.generation,c.previous_certificate_sha256]));}
function record(action,details={}){evidence.receipts.push({at:new Date().toISOString(),action,...details});writeFileSync(join(session.data,'manifest-live-evidence.json'),JSON.stringify(evidence,null,2));console.log(`[manifest] ${action}`);}
async function call(member,path,{method='GET',body,expected=200}={}){
  const response=await fetch(new URL(path,session.url),{method,headers:{authorization:`Bearer ${member.identity.token}`,'content-type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(30000)});
  const raw=await response.text();let value;try{value=raw?JSON.parse(raw):null;}catch{value={error:raw};}
  if(expected!==null)assert.equal(response.status,expected,`${method} ${path}: ${response.status}${response.ok?'':` ${value?.error??raw}`}`);
  return {status:response.status,value};
}
function crypt(key,plaintext,aad){const nonce=randomBytes(12),c=createCipheriv('chacha20-poly1305',key,nonce,{authTagLength:16});c.setAAD(aad,{plaintextLength:plaintext.length});return Buffer.concat([nonce,c.update(plaintext),c.final(),c.getAuthTag()]);}
function decrypt(key,blob,aad){const c=createDecipheriv('chacha20-poly1305',key,blob.subarray(0,12),{authTagLength:16});c.setAAD(aad,{plaintextLength:blob.length-28});c.setAuthTag(blob.subarray(-16));return Buffer.concat([c.update(blob.subarray(12,-16)),c.final()]);}
function seal(plain,recipient){const ephemeral=generateKeyPairSync('x25519').privateKey,epk=publicRaw(ephemeral),shared=diffieHellman({privateKey:ephemeral,publicKey:xPublic(recipient)}),key=hkdfSync('sha256',shared,Buffer.concat([epk,recipient]),'annex-e2e-seal-v1',32);return Buffer.concat([epk,crypt(key,plain,epk)]);}
function open(blob,secret){const epk=blob.subarray(0,32),recipient=publicRaw(secret),shared=diffieHellman({privateKey:secret,publicKey:xPublic(epk)});return decrypt(hkdfSync('sha256',shared,Buffer.concat([epk,recipient]),'annex-e2e-seal-v1',32),blob.subarray(32),epk);}
let channel,authority,head={head_seq:0,head_hash:''};
function event(member,operation){const e={version:1,authority_identity:authority,channel_id:channel,seq:head.head_seq+1,previous_hash:head.head_hash,operation,signer_pseudonym_id:member.certificate.pseudonym_id,signer_device_id:member.certificate.device_id,signer_certificate_sha256:certificateHash(member.certificate)};const input=text(['annex-channel-manifest-v1',1,authority,channel,e.seq,e.previous_hash,operation,e.signer_pseudonym_id,e.signer_device_id,e.signer_certificate_sha256]);e.event_hash=hash(input);e.signature_hex=sign(null,Buffer.from(input),member.signing).toString('hex');assert(verify(null,Buffer.from(input),createPublicKey(member.signing),Buffer.from(e.signature_hex,'hex')));return e;}
async function append(member,operation){const e=event(member,operation);const {value}=await call(member,`/api/channels/${channel}/key-manifest`,{method:'POST',body:e});assert.equal(value.applied,true);assert.equal(value.manifest.head_hash,e.event_hash);head=value.manifest;return value;}
function wrapRequest(member,directory,cek){const epoch=directory.required_epoch,key_id=hash(Buffer.concat([Buffer.from('annex-channel-key-v2\0'),cek]));const wraps=directory.member_keys.map(d=>({recipient_pseudonym_id:d.pseudonym_id,recipient_device_id:d.device_id,wrapped_key_b64:seal(Buffer.from(JSON.stringify(['annex-key-wrap-v2',authority,channel,epoch,key_id,d.pseudonym_id,d.device_id,cek.toString('base64')])),Buffer.from(d.x25519_pub_hex,'hex')).toString('base64')}));const digests=wraps.map(w=>{const d=directory.member_keys.find(d=>d.pseudonym_id===w.recipient_pseudonym_id&&d.device_id===w.recipient_device_id);return[w.recipient_pseudonym_id,w.recipient_device_id,d.certificate_sha256,hash(Buffer.from(w.wrapped_key_b64,'base64'))];}).sort((a,b)=>a[0].localeCompare(b[0])||a[1].localeCompare(b[1]));const input=text(['annex-channel-key-authorization-v1',authority,channel,member.certificate.pseudonym_id,member.certificate.device_id,certificateHash(member.certificate),head.head_seq,head.head_hash,'commit_epoch',directory.revision,epoch,key_id,digests]);return {version:2,operation:'commit_epoch',expected_revision:directory.revision,epoch,key_id,sender_device_id:member.certificate.device_id,wraps,authorization:{manifest_seq:head.head_seq,manifest_hash:head.head_hash,signature_hex:sign(null,Buffer.from(input),member.signing).toString('hex')}};}
async function socket(member){const url=new URL('/ws',session.url);url.protocol='ws:';url.searchParams.set('token',member.identity.token);const ws=new WebSocket(url);const frames=[];ws.addEventListener('message',e=>frames.push(JSON.parse(String(e.data))));await new Promise((res,rej)=>{const t=setTimeout(()=>rej(new Error('Socket open timeout')),10000);ws.addEventListener('open',()=>{clearTimeout(t);res();},{once:true});ws.addEventListener('error',()=>rej(new Error('Socket open failed')),{once:true});});const socket={ws,frames,send:frame=>ws.send(JSON.stringify(frame))};sockets.push(socket);return socket;}
async function wait(socket,predicate,label){const deadline=Date.now()+15000;while(Date.now()<deadline){const f=socket.frames.find(predicate);if(f)return f;await new Promise(r=>setTimeout(r,100));}throw new Error(`No ${label}; frame types ${socket.frames.map(f=>f.type).join(',')}`);}

try{
  channel=`manifest-live-${Date.now()}`;
  const alice=members[0],bob=members[1];authority=alice.certificate.server_identity;
  for(const member of members)await authenticate(member.identity);
  await proofRequest(session.url,'/api/channels',{method:'POST',token:alice.identity.token,body:{channel_id:channel,name:channel,channel_type:'Text',federation_scope:'Local',retention_days:null}});
  for(const member of members)await proofRequest(session.url,`/api/channels/${channel}/join`,{method:'POST',token:member.identity.token,body:{}});
  await append(alice,{kind:'genesis',join_policy:'eligible_self_join',history_policy:'all_members',certificate:alice.certificate});
  await append(bob,{kind:'join',certificate:bob.certificate});
  const directory=(await call(alice,`/api/channels/${channel}/member-keys`)).value;
  assert.equal(directory.member_keys.length,2);assert.equal(directory.authority_identity,authority);assert.equal(directory.e2e_enabled,true);
  record('Actual certified participants established and joined the member-signed manifest',{channel,head_seq:head.head_seq});
  const candidates=[randomBytes(32),randomBytes(32)].map(cek=>({cek,request:wrapRequest(alice,directory,cek)}));
  const commits=await Promise.all(candidates.map(c=>call(alice,`/api/channels/${channel}/key-wraps`,{method:'POST',body:c.request,expected:null})));
  assert.deepEqual(commits.map(c=>c.status).sort(),[200,409]);
  const winner=candidates[commits.findIndex(c=>c.status===200)];
  for(const member of members){const page=(await call(member,`/api/channels/${channel}/key-wraps?device_id=${member.certificate.device_id}`)).value;const wrap=page.wraps.find(w=>w.epoch===winner.request.epoch);assert.equal(wrap.authority_namespace,authority);assert(wrap.authorization);const fields=JSON.parse(open(Buffer.from(wrap.wrapped_key_b64,'base64'),member.secret));assert.deepEqual(fields,['annex-key-wrap-v2',authority,channel,winner.request.epoch,winner.request.key_id,member.certificate.pseudonym_id,member.certificate.device_id,winner.cek.toString('base64')]);}
  record('Concurrent independently signed key commits produced one winner; both real devices opened the persisted winning key',{epoch:winner.request.epoch,key_id:winner.request.key_id,statuses:commits.map(c=>c.status)});
  const sa=await socket(alice),sb=await socket(bob);for(const s of [sa,sb])s.send({type:'subscribe',channelId:channel});
  // Subscribe has no acknowledgement in the actual protocol. A typing frame
  // received through the channel proves the peer's subscription is installed.
  const typing=setInterval(()=>sa.send({type:'typing',channelId:channel}),250);
  try{await wait(sb,f=>f.type==='typing'&&f.channelId===channel,'actual channel subscription delivery');}finally{clearInterval(typing);}
  const plain=`Genuine encrypted signed-manifest session ${randomBytes(8).toString('hex')}`;
  const aad=Buffer.from(JSON.stringify(['annex-message-v2',authority,channel,winner.request.epoch,winner.request.key_id]));
  const encrypted=`e2e2:${winner.request.epoch}:${winner.request.key_id}:${crypt(winner.cek,Buffer.from(plain),aad).toString('base64')}`;
  sa.send({type:'message',channelId:channel,content:encrypted,clientRequestId:`${channel}-original`});
  const delivered=await wait(sb,f=>f.type==='message'&&f.content===encrypted,'encrypted message at recipient');
  assert.equal(decrypt(winner.cek,Buffer.from(delivered.content.split(':')[3],'base64'),aad).toString(),plain);
  await call(alice,`/api/channels/${channel}/e2e`,{method:'PUT',body:{enabled:false},expected:403});
  record('Real WebSocket encrypted message decrypted at recipient; unsigned encryption downgrade refused',{message_id:delivered.messageId,ciphertext_sha256:hash(encrypted)});
  await append(alice,{kind:'set_role',pseudonym_id:bob.certificate.pseudonym_id,role:'moderator'});
  await append(alice,{kind:'leave'});
  const afterLeave=(await call(bob,`/api/channels/${channel}/key-status`)).value;
  assert.equal(afterLeave.rotation_required,true);
  const before=sb.frames.length;sb.send({type:'message',channelId:channel,content:encrypted,clientRequestId:`${channel}-stale-after-signed-leave`});
  await wait({frames:sb.frames},f=>f.type==='error'&&/rotation|epoch|manifest/.test(JSON.stringify(f)),'stale epoch rejection');
  const history=await proofRequest(session.url,`/api/channels/${channel}/messages?limit=100`,{token:bob.identity.token});
  assert.equal(history.filter(m=>m.content===encrypted).length,1);
  assert(sb.frames.length>before);
  await append(alice,{kind:'join',certificate:alice.certificate});
  await append(bob,{kind:'set_encryption',enabled:false});
  assert.equal((await call(bob,`/api/channels/${channel}/key-status`)).value.e2e_enabled,false);
  await append(bob,{kind:'set_encryption',enabled:true});
  assert.equal((await call(bob,`/api/channels/${channel}/key-status`)).value.rotation_required,true);
  record('Signed role transfer, departure, rejoin and disable/enable controlled actual mode; departure blocked stale-epoch publication without deleting history',{head_seq:head.head_seq});
  evidence.complete=true;record('PASS');
}catch(error){record('FAILED',{error:error.message});process.exitCode=1;}
finally{for(const s of sockets)s.ws.close();writeFileSync(join(session.data,'manifest-live-evidence.json'),JSON.stringify(evidence,null,2));}
process.exit(process.exitCode??0);
