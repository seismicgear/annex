// Current ordered-authority and ordinary-attachment acceptance. All identities,
// certificates, joins, messages and files use real public production endpoints.
// Parent callbacks inspect captured storage read-only and control real processes.
import assert from 'node:assert/strict';
import { createPublicKey, randomUUID, verify } from 'node:crypto';
import { authenticate, createChannel, digest, history, joinChannel, request, sessionHeaders, socket, until } from './protocol.mjs';
import { certificateDigest, makeDeviceCertificate } from './device-certificates.mjs';

const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const encoded = value => JSON.stringify(canonical(value));
function verifyEvent(event, key, authority, channel) {
  const h = event.header;
  assert.equal(h.authority_identity, authority); assert.equal(h.channel_id, channel); assert.equal(h.version, 1);
  assert.equal(h.payload_hash, digest(encoded(event.payload)));
  const payload = encoded(['annex-channel-authority-event-v1', h.version, authority, channel,
    h.seq, h.previous_hash, h.event_kind, h.payload_hash, h.created_at]);
  assert.equal(event.event_hash, digest(payload));
  const publicKey = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(key, 'hex')]), format: 'der', type: 'spki' });
  assert(verify(null, Buffer.from(payload), publicKey, Buffer.from(event.signature, 'hex')), 'Independent accepted authority-event signature');
}
async function certificate(identity) {
  await authenticate(identity);
  const context = await request(identity.url, '/api/keys/certificate-context', { token: identity.token });
  assert.equal(context.server_identity, `annex-server:${digest(Buffer.from(context.server_ed25519_pub_hex, 'hex'))}`);
  const device = await makeDeviceCertificate(identity, context);
  const published = await request(identity.url, '/api/keys/me', { method: 'PUT', token: identity.token,
    body: { version: 2, x25519_pub_hex: device.certificate.x25519_pub_hex, device_certificate: device.certificate } });
  assert.equal(published.created, true);
  const directory = await request(identity.url, `/api/keys/${identity.pseudonym}`, { token: identity.token });
  assert.deepEqual(directory.devices.find(item => item.device_id === device.certificate.device_id).device_certificate, device.certificate);
  return { ...device, context };
}

export async function runChannelAuthorityAcceptance({ hostFounder, vmFounder, publicKeys, record, setPeerOnline, readRows, readUploadBytes }) {
  assert.equal(typeof readRows, 'function'); assert.equal(typeof readUploadBytes, 'function');
  const founders = [hostFounder, vmFounder]; const sockets = []; let replicaOffline = false;
  const channel = `authority-live-${randomUUID().slice(0, 12)}`;
  const emit = (label, details = {}) => record(`Channel authority: ${label}`, details);
  try {
    const devices = [];
    for (let i = 0; i < 2; i++) {
      devices.push(await certificate(founders[i]));
      assert.equal(devices[i].context.server_ed25519_pub_hex, publicKeys[i]);
    }
    const authority = devices[0].context.server_identity;
    await createChannel(hostFounder, channel); await joinChannel(hostFounder, channel);
    const before = await request(hostFounder.url, `/api/channels/${channel}/authority`, { token: hostFounder.token });
    assert.equal(before.initialized, false); assert.equal(before.authority_identity, authority);
    assert.equal((await request(hostFounder.url, `/api/channels/${channel}/authority/initialize`, {
      method: 'POST', token: hostFounder.token, body: { expected_authority_identity: authority },
    })).status, 'ok');
    const peers = await request(vmFounder.url, '/api/federation/authority-peers', { token: vmFounder.token });
    const peer = peers.peers.find(value => value.base_url === hostFounder.url);
    assert(peer); assert.equal(peer.authority_identity, authority);
    const join = await request(vmFounder.url, `/api/federation/channels/${channel}/attach`, { method: 'POST', token: vmFounder.token,
      body: { peer_id: peer.peer_id, authority_identity: authority, device_id: devices[1].certificate.device_id } });
    assert.equal(join.event.record_type, 'present'); verifyEvent(join.event, publicKeys[0], authority, channel);
    assert.equal(join.event.payload.action, 'member_joined'); assert.equal(join.event.payload.pseudonym_id, vmFounder.pseudonym);
    assert.equal(join.event.payload.requesting_identity, devices[1].context.server_identity);
    const membership = readRows(hostFounder.url,
      'SELECT certificate_sha256,expires_at FROM federated_human_memberships WHERE channel_id=? AND pseudonym_id=? AND device_id=?',
      [channel, vmFounder.pseudonym, devices[1].certificate.device_id]);
    assert.equal(membership.length, 1); assert.equal(membership[0].certificate_sha256, certificateDigest(devices[1].certificate));
    assert(membership[0].expires_at > Date.now() / 1000);
    for (const founder of founders) { const connection = await socket(founder); connection.send({ type: 'subscribe', channelId: channel }); sockets.push(connection); }
    emit('Genuine certified HUMAN attachment accepted under independently verified pinned authority signature', { channel, authority, acceptedSequence: join.accepted_seq });

    const send = async (source, content, attachmentIds = []) => {
      const id = randomUUID(); sockets[source].send({ type: 'message', channelId: channel, content, attachmentIds, clientRequestId: id });
      const answer = await sockets[source].wait(frame => frame.clientRequestId === id, 'actual authority-correlated message acknowledgement');
      assert.equal(answer.type, 'message'); assert.equal(answer.content, content); assert.equal(answer.senderPseudonym, founders[source].pseudonym);
      await sockets[1 - source].wait(frame => frame.type === 'message' && frame.messageId === answer.messageId && frame.content === content,
        'independent authority/replica WebSocket delivery');
      for (const founder of founders) assert.equal((await history(founder, channel)).filter(item => item.message_id === answer.messageId && item.content === content).length, 1);
      return answer;
    };
    const attachments = [];
    for (let source = 0; source < 2; source++) {
      const founder = founders[source], id = randomUUID(); const bytes = Buffer.from(`real-${source}-small-file\n`);
      assert(bytes.length < 28, 'This case proves the ordinary-size protocol independently of AEAD overhead');
      // Refresh through a real harmless authenticated read before issuing a
      // binary request with the exact current proof/bearer in memory.
      await request(founder.url, `/api/channels/${channel}/upload-policy`, { token: founder.token });
      const form = new FormData(); form.append('attachment_id', id); form.append('file', new Blob([bytes], { type: 'text/plain' }), `ordinary-${source}.txt`);
      const uploaded = await fetch(`${founder.url}/api/channels/${channel}/upload`, {
        method: 'POST', headers: sessionHeaders(founder.token), body: form, signal: AbortSignal.timeout(30_000),
      });
      assert.equal(uploaded.status, 200, 'Actual ordinary multipart upload');
      const receipt = await uploaded.json(); assert.equal(receipt.upload_id, id); assert.equal(receipt.encrypted, false);
      const message = await send(source, `[ordinary-${source}.txt](${receipt.url})`, [id]);
      const grants = [], reads = [], storedPaths = [];
      for (let target = 0; target < 2; target++) {
        const grant = await request(founders[target].url, '/api/uploads/grant', { method: 'POST', token: founders[target].token,
          body: { device_id: devices[target].certificate.device_id } });
        grants.push(grant.token);
        const url = new URL(receipt.url, founders[target].url); url.searchParams.set('t', grant.token);
        const downloaded = await until(async () => {
          const response = await fetch(url, { signal: AbortSignal.timeout(30_000) });
          if (response.status !== 200) { assert([404, 409, 503].includes(response.status), `Actual pending attachment read status ${response.status}`); return null; }
          return Buffer.from(await response.arrayBuffer());
        }, 'real bounded origin/replica attachment transfer and admitted read', 90_000);
        assert.deepEqual(downloaded, bytes);
        const stored = readRows(founders[target].url,
          'SELECT ciphertext_sha256,size_bytes,encrypted,lifecycle,purpose,storage_path FROM uploads WHERE upload_id=?', [id]);
        assert.equal(stored.length, 1); assert.equal(stored[0].ciphertext_sha256, digest(bytes)); assert.equal(stored[0].size_bytes, bytes.length);
        assert.equal(stored[0].encrypted, 0); assert.equal(stored[0].lifecycle, 'ready');
        assert.equal(stored[0].purpose, target === source ? 'chat' : 'federated_chat');
        storedPaths.push(stored[0].storage_path);
        assert.deepEqual(readUploadBytes(founders[target].url, id), bytes);
        assert.equal(readRows(founders[target].url, 'SELECT COUNT(*) AS n FROM message_uploads WHERE message_id=? AND upload_id=?', [message.messageId, id])[0].n, 1);
        reads.push({ server: founders[target].url, bytes: bytes.length, sha256: digest(bytes) });
      }
      attachments.push({ source, id, message, url: receipt.url, bytes, grants, storedPaths });
      emit('Ordinary bytes published, signed, associated, actually transferred and independently read at both peers', { attachmentId: id, messageId: message.messageId, reads });
    }

    const edited = `${channel}-edited-by-remote-human`, editId = randomUUID();
    sockets[1].send({ type: 'edit_message', channelId: channel, messageId: attachments[1].message.messageId,
      content: `${edited}\n[ordinary-1.txt](${attachments[1].url})`, clientRequestId: editId });
    // Edits and deletions acknowledge the persisted message ID; only errors
    // echo clientRequestId. Match the actual wire contract, as the client does.
    const edit = await sockets[1].wait(frame => frame.clientRequestId === editId ||
      (frame.type === 'message_edited' && frame.messageId === attachments[1].message.messageId &&
       frame.content === `${edited}\n[ordinary-1.txt](${attachments[1].url})`), 'actual remote-human edit acknowledgement');
    assert.equal(edit.type, 'message_edited');
    await sockets[0].wait(frame => frame.type === 'message_edited' && frame.messageId === edit.messageId && frame.content === edit.content, 'independent remote-human edit');
    for (const founder of founders) {
      const audit = await request(founder.url, `/api/channels/${channel}/messages/${edit.messageId}/edits`, { token: founder.token });
      assert(audit.some(item => item.old_content === attachments[1].message.content));
      assert.equal(readRows(founder.url, 'SELECT COUNT(*) AS n FROM message_uploads WHERE message_id=? AND upload_id=?', [edit.messageId, attachments[1].id])[0].n, 1);
    }
    emit('Remote human edit retains original attachment association and both actual audit histories');

    await request(vmFounder.url, `/api/channels/${channel}/leave`, { method: 'POST', token: vmFounder.token, body: {} });
    await request(vmFounder.url, `/api/channels/${channel}/messages?limit=100`, { token: vmFounder.token, expected: 403 });
    for (const attachment of attachments) {
      const url = new URL(attachment.url, vmFounder.url); url.searchParams.set('t', attachment.grants[1]);
      const denied = await fetch(url, { signal: AbortSignal.timeout(30_000) }); assert([403, 404].includes(denied.status), 'Saved device-bound grant must not outlive actual channel membership');
    }
    assert.equal(readRows(hostFounder.url, 'SELECT COUNT(*) AS n FROM channel_members WHERE channel_id=? AND pseudonym_id=?', [channel, vmFounder.pseudonym])[0].n, 0);
    await request(vmFounder.url, `/api/channels/${channel}/join`, { method: 'POST', token: vmFounder.token, body: {} });
    sockets[1].send({ type: 'subscribe', channelId: channel });
    emit('Real remote Leave revokes fresh history and saved attachment grants; ordinary certified rejoin succeeds');

    for (const attachment of attachments) {
      const source = attachment.source, id = randomUUID();
      sockets[source].send({ type: 'delete_message', channelId: channel, messageId: attachment.message.messageId, clientRequestId: id });
      const removed = await sockets[source].wait(frame => frame.clientRequestId === id ||
        (frame.type === 'message_deleted' && frame.messageId === attachment.message.messageId), 'actual author deletion committed at authority');
      assert.equal(removed.type, 'message_deleted');
      await sockets[1 - source].wait(frame => frame.type === 'message_deleted' && frame.messageId === attachment.message.messageId, 'independent deletion event');
      for (let target = 0; target < 2; target++) {
        await until(() => readRows(founders[target].url, 'SELECT COUNT(*) AS n FROM uploads WHERE upload_id=?', [attachment.id])[0].n === 0,
          'actual origin/replica upload metadata cleanup', 60_000);
        const url = new URL(attachment.url, founders[target].url); url.searchParams.set('t', attachment.grants[target]);
        const denied = await fetch(url, { signal: AbortSignal.timeout(30_000) }); assert([403, 404].includes(denied.status), 'Deleted file cannot be read with a saved grant');
        assert.equal(readRows(founders[target].url, 'SELECT COUNT(*) AS n FROM message_uploads WHERE message_id=?', [attachment.message.messageId])[0].n, 0);
        await until(() => readUploadBytes(founders[target].url, attachment.id, attachment.storedPaths[target]) === null,
          'actual deletion worker removes the previously independently read file', 60_000);
      }
    }
    emit('Author deletions remove both attachment associations and actual disk files, and refuse saved-grant reads on both peers');

    await setPeerOnline(vmFounder.url, false); replicaOffline = true;
    await until(() => sockets[1].ws.readyState === WebSocket.CLOSED, 'real replica process stopped');
    const delayedId = randomUUID(), delayedText = `${channel}-authority-while-replica-offline`;
    sockets[0].send({ type: 'message', channelId: channel, content: delayedText, clientRequestId: delayedId });
    const delayed = await sockets[0].wait(frame => frame.clientRequestId === delayedId, 'authority commits during actual replica outage');
    assert.equal(delayed.type, 'message');
    await setPeerOnline(vmFounder.url, true); replicaOffline = false;
    await until(async () => (await history(vmFounder, channel)).some(item => item.message_id === delayed.messageId && item.content === delayedText),
      'signed ordered catchup after real same-disk replica restart', 90_000);
    sockets[1] = await socket(vmFounder); sockets[1].send({ type: 'subscribe', channelId: channel });
    const aHead = await request(hostFounder.url, `/api/channels/${channel}/authority`, { token: hostFounder.token });
    const bHead = await request(vmFounder.url, `/api/channels/${channel}/authority`, { token: vmFounder.token });
    assert.equal(bHead.head_hash, aHead.head_hash); assert.equal(bHead.head_seq, aHead.head_seq);
    emit('Actual replica shutdown and restart resumes signed ordered history from persisted head', { messageId: delayed.messageId, headSequence: bHead.head_seq });

    return { complete: true, channelId: channel, scope: 'Actual certified-human ordered authority and ordinary attachments; encrypted federation is a separate required lane' };
  } finally {
    for (const connection of sockets) connection.ws.close();
    if (replicaOffline) await setPeerOnline(vmFounder.url, true);
  }
}
