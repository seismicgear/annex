// Five actual Annex processes, including an Ubuntu peer. Only operator public
// trust anchors are provisioned directly; agents, agreements, policy and RTX
// traffic must use production APIs. Usage: node mesh.mjs <session A> ... <E>.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { execFileSync } from 'node:child_process';
import { createHash, createPublicKey, randomBytes, randomUUID, verify } from 'node:crypto';
import { register, request, handshake, socket, signingIdentityFromSeed, until } from './protocol.mjs';
import { authorSigningPayload } from './rtx.mjs';

const paths = process.argv.slice(2);
assert.equal(paths.length, 5, 'Five isolated session records are required to prove the three-hop boundary');
const nodes = paths.map(path => JSON.parse(readFileSync(path, 'utf8').replace(/^\uFEFF/, '')));
assert.equal(new Set(nodes.map(n => n.url)).size, 5);
const dataChannelRequired = Boolean(nodes[0].signal_base_url);
assert(nodes.every(node => Boolean(node.signal_base_url) === dataChannelRequired), 'All mesh peers must select the same acceptance transport');
const guest = (command, ...args) => execFileSync('wsl.exe', ['-d','Ubuntu','-u','root','--exec',command,...args], { encoding:'utf8' });
const read = (node, path) => node.data.startsWith('/') ? guest('cat', path) : readFileSync(path, 'utf8');
const receipts = [];
const connections = [];
const sha = input => createHash('sha256').update(input).digest();
const field = input => `${Buffer.byteLength(input)}:${input}\n`;
const u64 = number => { const value=Buffer.alloc(8); value.writeBigUInt64LE(BigInt(number)); return value; };
const absorb = (hash, input) => { const bytes=Buffer.from(input); hash.update(u64(bytes.length)); hash.update(bytes); };
function query(node, sql, params = [], mutate = false) {
  const dbPath = node.data.startsWith('/') ? `${node.data}/annex.db` : join(node.data, 'annex.db');
  if(node.data.startsWith('/')) {
    return JSON.parse(guest('python3','-c',
      'import sqlite3,json,sys; c=sqlite3.connect(sys.argv[1]); c.row_factory=sqlite3.Row; r=c.execute(sys.argv[2],json.loads(sys.argv[3])); rows=[dict(x) for x in r] if r.description else []; c.commit(); print(json.dumps(rows))',
      dbPath,sql,JSON.stringify(params)));
  }
  const db=new DatabaseSync(dbPath,{readOnly:!mutate});
  try { db.exec('PRAGMA busy_timeout=5000'); return mutate ? db.prepare(sql).run(...params) : db.prepare(sql).all(...params); }
  finally { db.close(); }
}
function record(action, details={}) { receipts.push({at:new Date().toISOString(),action,...details}); console.log(`[mesh-live] ${action}`); }
function verifySignature(key, signature, bytes) {
  const publicKey=createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(key,'hex')]),format:'der',type:'spki'});
  assert(verify(null,Buffer.from(bytes),publicKey,Buffer.from(signature,'hex')), 'Independent Ed25519 verification failed');
}
function invariantDigest(bundle) {
  const v1=createHash('sha256');
  for(const value of ['annex/rtx/scope-invariant/v1',bundle.bundle_id,bundle.source_pseudonym,bundle.source_server,bundle.summary,String(bundle.created_at),bundle.signature,bundle.vrp_handshake_ref]) absorb(v1,value);
  const v2=createHash('sha256').update('annex/rtx/scope-invariant/v2\n').update(v1.digest('hex'));
  for(const vector of [bundle.domain_tags,bundle.caveats]) { v2.update(u64(vector.length)); for(const value of vector) absorb(v2,value); }
  return v2.digest('hex');
}
function contentHash(bundle) {
  const hash=createHash('sha256');
  for(const value of [bundle.bundle_id,bundle.source_pseudonym,bundle.source_server,bundle.summary,bundle.reasoning_chain??'']) absorb(hash,value);
  for(const vector of [bundle.domain_tags,bundle.caveats]) { hash.update(u64(vector.length)); for(const value of vector) absorb(hash,value); }
  for(const value of [String(bundle.created_at),bundle.signature,bundle.vrp_handshake_ref]) absorb(hash,value);
  return hash.digest('hex');
}
function verifyProvenance(frame, receiver, author, original) {
  const p=frame.provenance, b=frame.bundle, o=p.origin;
  assert.equal(p.origin_server,author.url); assert.equal(p.bundle_id,b.bundle_id);
  assert.equal(o.version,2); assert.equal(o.max_hops,3);
  assert.equal(o.reasoning_commitment,sha(original.reasoning_chain??'').toString('hex'));
  const originPayload='annex/rtx/origin-attestation/v2\n'+field(b.bundle_id)+field(author.url)+field(b.source_pseudonym)+`${b.created_at}\n`+field(b.vrp_handshake_ref)+field(invariantDigest(b))+field(o.reasoning_commitment)+`${o.max_hops}\n`;
  verifySignature(author.key.publicKey,o.signature,sha(originPayload));
  let previous='';
  assert(p.hops.length>0 && p.hops.length<=3);
  assert.equal(new Set(p.hops.map(h=>h.server)).size,p.hops.length,'No repeated relay in a valid chain');
  assert(!p.hops.some(h=>h.server===receiver.url),'Receiver must not already occur in its chain');
  for(const [index,hop] of p.hops.entries()) {
    const sender=nodes.find(n=>n.url===hop.server); assert(sender);
    const next=p.hops[index+1]?.server??receiver.url;
    const payload='annex/rtx/hop/v1\n'+field(b.bundle_id)+field(author.url)+field(o.signature)+`${index}\n`+field(hop.server)+field(next)+field(hop.content_hash)+field(previous);
    verifySignature(sender.key.publicKey,hop.signature,sha(payload)); previous=sha(payload).toString('hex');
  }
  // A receiving server may strip reasoning after verifying the incoming hop.
  // The signed inbound hash must identify either the known original content
  // or that exact content with reasoning removed, never arbitrary mutations.
  assert([contentHash(original),contentHash({...original,reasoning_chain:null})].includes(p.hops.at(-1).content_hash));
}
async function setPolicy(node, full) {
  node.policy={...node.policy,allow_reflection_summaries:true,allow_full_knowledge:full};
  await request(node.url,'/api/admin/policy',{method:'PUT',token:node.founder.token,body:node.policy});
  assert.equal((await request(node.url,'/api/admin/policy',{token:node.founder.token})).allow_full_knowledge,full);
}
async function agreement(receiver, sender) {
  const payload=handshake(sender.policy);
  const response=await request(receiver.url,'/api/federation/handshake',{method:'POST',body:{base_url:sender.url,...payload,signature:sender.key.sign(`${sender.url}\n${JSON.stringify(payload)}`)}});
  assert.equal(response.alignment_status,'Aligned');
  // Each direction records the accepting server's own permission. The anchor
  // handshake does not advertise the sender's independent outbound policy.
  assert.equal(response.transfer_scope,receiver.policy.allow_full_knowledge?'FullKnowledgeBundle':'ReflectionSummariesOnly');
}
async function edge(a,b) { await agreement(a,b); await agreement(b,a); }
async function align(node, identity=node.agent, key=node.agentKey) {
  const report=await request(node.url,'/api/vrp/agent-handshake',{method:'POST',token:identity.token,body:{pseudonymId:identity.pseudonym,handshake:handshake(node.policy),signingPubkey:key.publicKey}});
  assert.equal(report.alignment_status,'Aligned'); if(identity===node.agent) node.handshakeRef=report.handshakeRef;
}
function bundle(node,label,reasoning) {
  const value={bundle_id:randomUUID(),source_pseudonym:node.agent.pseudonym,source_server:node.url,domain_tags:['annex-mesh-acceptance','systems'],summary:`Actual mesh: ${label}; π`,reasoning_chain:reasoning,caveats:['Disposable acceptance content.'],created_at:Date.now(),signature:'',vrp_handshake_ref:node.handshakeRef};
  value.signature=node.agentKey.sign(sha(authorSigningPayload(value))); return value;
}
async function publish(node,value) {
  const result=await request(node.url,'/api/rtx/publish',{method:'POST',token:node.agent.token,body:value}); assert.equal(result.bundleId,value.bundle_id);
}
async function delivered(node,author,value,reasoning,expectedHops) {
  const frame=await node.connection.wait(f=>f.type==='rtx_bundle'&&f.bundle?.bundle_id===value.bundle_id,`actual mesh bundle at ${node.url}`);
  assert.deepEqual(frame.bundle,{...value,reasoning_chain:reasoning});
  if(node!==author) { assert.equal(frame.federated,true); verifyProvenance(frame,node,author,value); if(expectedHops!==undefined) assert.equal(frame.provenance.hops.length,expectedHops); }
  else verifySignature(node.agentKey.publicKey,value.signature,sha(authorSigningPayload(value)));
  await until(()=>query(node,'SELECT bundle_id FROM rtx_bundles WHERE bundle_id=?',[value.bundle_id]).length===1,'durable mesh bundle');
  const row=query(node,'SELECT summary,reasoning_chain,domain_tags_json,caveats_json,provenance_json FROM rtx_bundles WHERE bundle_id=?',[value.bundle_id])[0];
  assert.equal(row.summary,value.summary); assert.equal(row.reasoning_chain,reasoning); assert.deepEqual(JSON.parse(row.domain_tags_json),value.domain_tags); assert.deepEqual(JSON.parse(row.caveats_json),value.caveats);
  if(node!==author) assert.deepEqual(JSON.parse(row.provenance_json),frame.provenance);
  return frame;
}
async function main() {
  for(const node of nodes) {
    assert.equal((await request(node.url,'/health')).status,'ok');
    assert.equal(query(node,'SELECT COUNT(*) AS n FROM federation_agreements')[0].n,0,'Fresh agreements required');
    assert.equal(query(node,'SELECT COUNT(*) AS n FROM platform_identities')[0].n,0,'Fresh identities required');
    const seedPath=node.data.startsWith('/')?`${node.data}/signing.key`:join(node.data,'signing.key');
    node.key=signingIdentityFromSeed(read(node,seedPath));
  }
  for(const node of nodes) for(const other of nodes) if(node!==other) query(node,"INSERT INTO instances(base_url,public_key,label,status) VALUES(?,?,'Mesh operator trust anchor','ACTIVE')",[other.url,other.key.publicKey],true);
  record('Five real server processes have public operator trust anchors and zero fixture agreements');
  for(const node of nodes) {
    node.founder=await register(node.url); node.policy=await request(node.url,'/api/admin/policy',{token:node.founder.token}); await setPolicy(node,true);
    node.agent=await register(node.url,2,`annex:mesh:${randomUUID()}`); node.agentKey=signingIdentityFromSeed(randomBytes(32).toString('hex')); await align(node);
    // Local publish intentionally excludes its author. A genuinely separate
    // observer proves local delivery without weakening that loop safeguard.
    node.observer=await register(node.url,2,`annex:mesh:observer:${randomUUID()}`);
    node.observerKey=signingIdentityFromSeed(randomBytes(32).toString('hex')); await align(node,node.observer,node.observerKey);
    node.connection=await socket(node.observer); connections.push(node.connection);
    await request(node.url,'/api/rtx/subscribe',{method:'POST',token:node.observer.token,body:{domain_filters:['annex-mesh-acceptance'],accept_federated:true}});
  }
  for(let index=0;index<4;index++) await edge(nodes[index],nodes[index+1]);
  for(const [index,node] of nodes.entries()) assert.equal(query(node,'SELECT COUNT(*) AS n FROM federation_agreements WHERE active=1')[0].n,index===0||index===4?1:2);
  record('Genuine founders, proof-authenticated agents and signed adjacent-only chain agreements established');
  const initial=bundle(nodes[0],'three-hop full knowledge','Signed chain content retained through three actual relays.'); await publish(nodes[0],initial);
  for(let index=0;index<4;index++) await delivered(nodes[index],nodes[0],initial,initial.reasoning_chain,index);
  const marker=bundle(nodes[3],'positive control at the fourth-hop boundary',null); await publish(nodes[3],marker); await delivered(nodes[4],nodes[3],marker,null,1);
  await new Promise(resolve=>setTimeout(resolve,1500));
  assert.equal(query(nodes[4],'SELECT COUNT(*) AS n FROM rtx_bundles WHERE bundle_id=?',[initial.bundle_id])[0].n,0);
  assert(!nodes[4].connection.frames.some(f=>f.bundle?.bundle_id===initial.bundle_id));
  record('One, two and three real relay hops independently verified; fourth hop refused with a live positive control',{bundleId:initial.bundle_id});
  await setPolicy(nodes[2],false); await edge(nodes[1],nodes[2]); await edge(nodes[2],nodes[3]); await align(nodes[2]); await align(nodes[2],nodes[2].observer,nodes[2].observerKey);
  const narrowed=bundle(nodes[0],'middle policy narrows transfer','This reasoning must disappear after the middle policy boundary.'); await publish(nodes[0],narrowed);
  const frames=[];
  for(let index=0;index<4;index++) frames.push(await delivered(nodes[index],nodes[0],narrowed,index<2?narrowed.reasoning_chain:null,index));
  assert.notEqual(frames[3].provenance.hops[0].content_hash,frames[3].provenance.hops[2].content_hash);
  record('Middle-server policy removes reasoning; unchanged origin commitment and every signed hop remain independently valid',{bundleId:narrowed.bundle_id});
  await edge(nodes[4],nodes[0]);
  const cyclic=bundle(nodes[0],'cycle exactly once',null); await publish(nodes[0],cyclic);
  for(const node of nodes) await delivered(node,nodes[0],cyclic,null);
  const barrier=bundle(nodes[3],'cycle observation barrier',null); await publish(nodes[3],barrier);
  for(const node of nodes) await delivered(node,nodes[3],barrier,null);
  await new Promise(resolve=>setTimeout(resolve,1500));
  for(const node of nodes) {
    assert.equal(node.connection.ws.readyState,WebSocket.OPEN);
    assert.equal(node.connection.frames.filter(f=>f.type==='rtx_bundle'&&f.bundle?.bundle_id===cyclic.bundle_id).length,1);
    assert.equal(query(node,'SELECT COUNT(*) AS n FROM rtx_bundles WHERE bundle_id=?',[cyclic.bundle_id])[0].n,1);
    await request(node.url,'/api/rtx/subscribe',{method:'DELETE',token:node.observer.token});
    assert.equal((await request(node.url,'/api/rtx/subscriptions',{token:node.observer.token})).subscription,undefined);
  }
  record('A real five-server cycle delivers exactly once at every subscriber; all subscriptions removed',{bundleId:cyclic.bundle_id});
  if (dataChannelRequired) {
    const operations = [];
    for (const node of nodes) {
      const lines = read(node, node.data.startsWith('/') ? `${node.data}/stdout.log` : join(node.data,'stdout.log'))
        .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '').split('\n')
        .filter(line => line.includes('federation operation acknowledged'));
      assert(lines.length > 0, `Actual data-channel operations required at ${node.url}`);
      assert(lines.every(line => line.includes('transport="webrtc"') && line.includes('receipt="verified"') && /bytes_sent=[1-9][0-9]*/.test(line) && /bytes_received=[1-9][0-9]*/.test(line)), 'Every mesh operation requires real bytes and a verified data-channel receipt; no HTTP fallback');
      const rtx = lines.filter(line => line.includes('endpoint="/api/federation/rtx"'));
      for (const adjacent of query(node,'SELECT remote_instance_id FROM federation_agreements WHERE active=1')) {
        assert(rtx.some(line => new RegExp(`peer_id=${adjacent.remote_instance_id}(?: |$)`).test(line)), `Actual RTX data-channel receipt for every adjacent direction at ${node.url}`);
      }
      operations.push({url:node.url,verifiedOperations:lines.length,verifiedRtx:rtx.length});
    }
    record('Every adjacent mesh direction completed RTX over DTLS/SCTP with verified receipts and zero HTTP fallback',{operations});
  }
}
let complete=false;
try { await main(); complete=true; }
catch(error) { record('FAILED',{error:error.message,stack:error.stack}); process.exitCode=1; }
finally {
  for(const connection of connections) connection.ws.close();
  const publicNodes=nodes.map(({url,data,binary_sha256,signal_base_url})=>({url,data,binary_sha256,signal_base_url}));
  writeFileSync(join(nodes[0].data,'mesh-live-evidence.json'),JSON.stringify({complete,transport:dataChannelRequired?'webrtc':'http',nodes:publicNodes,boundary:'Actual separate processes across Windows and Ubuntu; operator public anchors only. Same physical computer, not WAN or independent physical machines.',receipts},null,2));
}
// snarkjs proving workers otherwise keep a finished CLI alive indefinitely.
process.exit(complete?0:1);
