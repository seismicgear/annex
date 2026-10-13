// Independent real cross-peer encryption acceptance. Private keys exist only
// in this process; all identities, certificates, manifests, wraps and uploads
// are accepted through production endpoints. No database mutations or mocks.
import assert from 'node:assert/strict';
import { createPublicKey, generateKeyPairSync, diffieHellman, hkdfSync, randomBytes,
  randomUUID, createCipheriv, createDecipheriv, verify, timingSafeEqual } from 'node:crypto';
import { authenticate, register, request, sessionHeaders, digest, createChannel, joinChannel,
  socket, history, until } from './protocol.mjs';
import { makeDeviceCertificate, certificateDigest } from './device-certificates.mjs';

const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const json = value => JSON.stringify(canonical(value));
const rawPublic = key => createPublicKey(key).export({ format: 'der', type: 'spki' }).subarray(-32);
const publicKey = (bytes, type) => createPublicKey({ key: Buffer.concat([
  Buffer.from(type === 'x25519' ? '302a300506032b656e032100' : '302a300506032b6570032100', 'hex'), bytes,
]), format: 'der', type: 'spki' });
function signature(key, input, signed) {
  assert(verify(null, Buffer.from(input), publicKey(Buffer.from(key, 'hex'), 'ed25519'), Buffer.from(signed, 'hex')),
    'Independent signature verification');
}
function sameSecret(a, b) { assert(a.length === b.length && timingSafeEqual(a, b), 'Secret bytes match without disclosure'); }
function encrypt(key, plain, aad) {
  const nonce = randomBytes(12), cipher = createCipheriv('chacha20-poly1305', key, nonce, { authTagLength: 16 });
  cipher.setAAD(aad, { plaintextLength: plain.length });
  return Buffer.concat([nonce, cipher.update(plain), cipher.final(), cipher.getAuthTag()]);
}
function decrypt(key, bytes, aad) {
  const cipher = createDecipheriv('chacha20-poly1305', key, bytes.subarray(0, 12), { authTagLength: 16 });
  cipher.setAAD(aad, { plaintextLength: bytes.length - 28 }); cipher.setAuthTag(bytes.subarray(-16));
  return Buffer.concat([cipher.update(bytes.subarray(12, -16)), cipher.final()]);
}
function seal(plain, recipient) {
  const secret = generateKeyPairSync('x25519').privateKey, ephemeral = rawPublic(secret);
  const shared = diffieHellman({ privateKey: secret, publicKey: publicKey(recipient, 'x25519') });
  const key = Buffer.from(hkdfSync('sha256', shared, Buffer.concat([ephemeral, recipient]), 'annex-e2e-seal-v1', 32));
  try { return Buffer.concat([ephemeral, encrypt(key, plain, ephemeral)]); } finally { key.fill(0); shared.fill(0); }
}
function open(bytes, secret) {
  const ephemeral = bytes.subarray(0, 32), recipient = rawPublic(secret);
  const shared = diffieHellman({ privateKey: secret, publicKey: publicKey(ephemeral, 'x25519') });
  const key = Buffer.from(hkdfSync('sha256', shared, Buffer.concat([ephemeral, recipient]), 'annex-e2e-seal-v1', 32));
  try { return decrypt(key, bytes.subarray(32), ephemeral); } finally { key.fill(0); shared.fill(0); }
}
const keyId = key => digest(Buffer.concat([Buffer.from('annex-channel-key-v2\0'), key]));
const manifestInput = event => json(['annex-channel-manifest-v1', 1, event.authority_identity, event.channel_id,
  event.seq, event.previous_hash, event.operation, event.signer_pseudonym_id, event.signer_device_id, event.signer_certificate_sha256]);
const wrapInput = value => json(['annex-channel-key-authorization-v1', value.authority_identity, value.channel_id,
  value.sender_pseudonym_id, value.sender_device_id, value.sender_certificate_sha256, value.manifest_seq, value.manifest_hash,
  value.operation, value.expected_revision, value.epoch, value.key_id, value.wrap_digests]);

export async function runEncryptedAuthorityAcceptance({ hostFounder, vmFounder, publicKeys,
  record, setPeerOnline, readRows, readUploadBytes }) {
  const founders = [hostFounder, vmFounder], devices = [], allCertificates = new Map(), connections = [];
  const keys = new Map(), attachments = [], events = [];
  const channel = `encrypted-federation-${randomUUID().slice(0, 12)}`;
  let authority, offline = null;
  const emit = (action, details = {}) => record(`Encrypted federation: ${action}`, details);
  const api = (node, route, options = {}) => request(founders[node].url, route, { token: founders[node].token, ...options });
  async function certify(node, previous = null) {
    const identity = founders[node]; await authenticate(identity);
    const context = await api(node, '/api/keys/certificate-context');
    assert.equal(context.server_ed25519_pub_hex, publicKeys[node]);
    assert.equal(context.server_identity, `annex-server:${digest(Buffer.from(publicKeys[node], 'hex'))}`);
    const device = await makeDeviceCertificate(identity, context, { previous });
    const accepted = await api(node, '/api/keys/me', { method: 'PUT', body: {
      version: 2, x25519_pub_hex: device.certificate.x25519_pub_hex, device_certificate: device.certificate,
    } });
    assert.equal(accepted.created, true); allCertificates.set(device.certificate.device_id, device.certificate);
    return { ...device, context };
  }
  function signEvent(node, operation) {
    const certificate = devices[node].certificate;
    const value = { version: 1, authority_identity: authority, channel_id: channel,
      seq: events.length + 1, previous_hash: events.at(-1)?.event_hash ?? '', operation,
      signer_pseudonym_id: certificate.pseudonym_id, signer_device_id: certificate.device_id,
      signer_certificate_sha256: certificateDigest(certificate) };
    const input = manifestInput(value);
    return { ...value, event_hash: digest(input), signature_hex: devices[node].signing.sign(input) };
  }
  async function verifyManifest(node) {
    const page = await api(node, `/api/channels/${channel}/key-manifest`);
    assert.equal(page.authority_identity, authority); assert.equal(page.channel_id, channel);
    assert.equal(page.next_after_seq, null); assert.equal(page.events.length, events.length);
    let previous = '';
    for (const [index, event] of page.events.entries()) {
      assert.equal(digest(json(event)), digest(json(events[index])), 'Unmodified actually signed event');
      assert.equal(event.previous_hash, previous); assert.equal(event.seq, index + 1);
      const certificate = allCertificates.get(event.signer_device_id); assert(certificate);
      assert.equal(event.signer_certificate_sha256, certificateDigest(certificate));
      const input = manifestInput(event); assert.equal(event.event_hash, digest(input));
      signature(certificate.ed25519_pub_hex, input, event.signature_hex); previous = event.event_hash;
    }
    assert.equal(page.head_hash, previous); return page;
  }
  async function append(node, operation, verifyNodes = [0, 1]) {
    const event = signEvent(node, operation);
    const response = await api(node, `/api/channels/${channel}/key-manifest`, { method: 'POST', body: event });
    assert.equal(response.applied, true); assert.equal(response.manifest.head_hash, event.event_hash);
    events.push(event);
    for (const peer of verifyNodes) await verifyManifest(peer);
    return response;
  }
  function admittedAt(count = events.length) {
    const members = new Map();
    for (const event of events.slice(0, count)) {
      const operation = event.operation;
      if (['genesis', 'join', 'admit_member'].includes(operation.kind)) {
        members.set(operation.certificate.pseudonym_id, new Set([operation.certificate.device_id]));
      } else if (operation.kind === 'recover_device') {
        assert(members.has(operation.certificate.pseudonym_id));
        members.set(operation.certificate.pseudonym_id, new Set([operation.certificate.device_id]));
      } else if (operation.kind === 'remove_member') members.delete(operation.pseudonym_id);
      else throw new Error('Acceptance verifier needs an explicit rule for this manifest operation');
    }
    return members;
  }
  function recipients(directory) {
    assert.equal(directory.authority_identity, authority); assert.equal(directory.e2e_enabled, true);
    const admitted = admittedAt();
    const expected = [...admitted].flatMap(([pid, ids]) => [...ids].map(id => `${pid}:${id}`)).sort();
    assert.deepEqual(directory.member_keys.map(row => `${row.pseudonym_id}:${row.device_id}`).sort(), expected,
      'Recipient set must equal the independently reconstructed member-signed admission lineage');
    for (const row of directory.member_keys) {
      const certificate = allCertificates.get(row.device_id); assert(certificate, 'No unrecognized server-selected recipient');
      assert.equal(digest(json(row.device_certificate)), digest(json(certificate)));
      assert.equal(row.certificate_sha256, certificateDigest(certificate));
      const member = directory.manifest.members.find(value => value.pseudonym_id === row.pseudonym_id);
      assert(member?.devices.some(value => value.device_id === row.device_id && value.certificate_sha256 === row.certificate_sha256));
    }
    assert.equal(directory.manifest.head_hash, events.at(-1).event_hash);
    return directory.member_keys;
  }
  function wraps(node, directory, key, operation = 'commit_epoch', epoch = directory.required_epoch, targets = recipients(directory)) {
    const id = keyId(key), certificate = devices[node].certificate;
    const wrapped = targets.map(row => ({ recipient_pseudonym_id: row.pseudonym_id, recipient_device_id: row.device_id,
      wrapped_key_b64: seal(Buffer.from(JSON.stringify(['annex-key-wrap-v2', authority, channel, epoch, id,
        row.pseudonym_id, row.device_id, key.toString('base64')])), Buffer.from(row.x25519_pub_hex, 'hex')).toString('base64') }));
    const tuples = wrapped.map(value => [value.recipient_pseudonym_id, value.recipient_device_id,
      certificateDigest(allCertificates.get(value.recipient_device_id)), digest(Buffer.from(value.wrapped_key_b64, 'base64'))])
      .sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0);
    const receipt = { authority_identity: authority, channel_id: channel, sender_pseudonym_id: certificate.pseudonym_id,
      sender_device_id: certificate.device_id, sender_certificate_sha256: certificateDigest(certificate),
      manifest_seq: events.length, manifest_hash: events.at(-1).event_hash, operation,
      expected_revision: directory.revision, epoch, key_id: id, wrap_digests: tuples };
    return { version: 2, operation, expected_revision: directory.revision, epoch, key_id: id,
      sender_device_id: certificate.device_id, wraps: wrapped, authorization: { manifest_seq: events.length,
        manifest_hash: events.at(-1).event_hash, signature_hex: devices[node].signing.sign(wrapInput(receipt)) } };
  }
  async function keyRead(node, epoch, expected) {
    const device = devices[node], page = await api(node, `/api/channels/${channel}/key-wraps?device_id=${device.certificate.device_id}`);
    const wrap = page.wraps.find(value => value.epoch === epoch); assert(wrap, 'Actual device-specific persisted wrap');
    const receipt = wrap.authorization; assert(receipt);
    assert.equal(wrap.authority_namespace, authority); assert.equal(receipt.authority_identity, authority);
    assert.equal(receipt.channel_id, channel); assert.equal(receipt.epoch, epoch); assert.equal(receipt.key_id, keyId(expected));
    assert.equal(receipt.manifest_hash, events[receipt.manifest_seq - 1].event_hash);
    const signer = allCertificates.get(receipt.sender_device_id); assert(signer);
    const admission = admittedAt(receipt.manifest_seq);
    assert(admission.get(receipt.sender_pseudonym_id)?.has(receipt.sender_device_id));
    assert.equal(receipt.sender_certificate_sha256, certificateDigest(signer));
    const input = wrapInput(receipt); assert.equal(receipt.authorization_sha256, digest(input)); signature(signer.ed25519_pub_hex, input, receipt.signature_hex);
    const tuple = receipt.wrap_digests.find(value => value[0] === founders[node].pseudonym && value[1] === device.certificate.device_id);
    assert(tuple); assert.equal(tuple[2], certificateDigest(device.certificate)); assert.equal(tuple[3], digest(Buffer.from(wrap.wrapped_key_b64, 'base64')));
    for (const [pid, id, certHash] of receipt.wrap_digests) {
      assert(admission.get(pid)?.has(id)); assert.equal(certificateDigest(allCertificates.get(id)), certHash);
    }
    const opened = open(Buffer.from(wrap.wrapped_key_b64, 'base64'), device.device.privateKey);
    const fields = JSON.parse(opened.toString('utf8'));
    assert.equal(digest(JSON.stringify(fields.slice(0, 7))), digest(JSON.stringify(['annex-key-wrap-v2', authority, channel,
      epoch, keyId(expected), founders[node].pseudonym, device.certificate.device_id])));
    const key = Buffer.from(fields[7], 'base64'); sameSecret(key, expected); opened.fill(0);
    return key;
  }
  async function commit(node, targets = [0, 1]) {
    const directory = await api(node, `/api/channels/${channel}/member-keys`), key = randomBytes(32);
    const body = wraps(node, directory, key), result = await api(node, `/api/channels/${channel}/key-wraps`, { method: 'POST', body });
    assert.equal(result.status, 'ok'); assert.equal(result.state.current_epoch, body.epoch); keys.set(body.epoch, key);
    for (const peer of targets) (await keyRead(peer, body.epoch, key)).fill(0);
    return { key, epoch: body.epoch, id: body.key_id, body };
  }
  const messageAad = key => Buffer.from(JSON.stringify(['annex-message-v2', authority, channel, key.epoch, key.id]));
  const bodyFor = (plain, key) => `e2e2:${key.epoch}:${key.id}:${encrypt(key.key, Buffer.from(plain), messageAad(key)).toString('base64')}`;
  async function message(node, plain, key, ids = []) {
    const body = bodyFor(plain, key), requestId = randomUUID();
    connections[node].send({ type: 'message', channelId: channel, content: body, attachmentIds: ids, clientRequestId: requestId });
    const accepted = await connections[node].wait(frame => frame.clientRequestId === requestId, 'actual encrypted publication acknowledgement');
    assert.equal(accepted.type, 'message'); assert.equal(accepted.content, body);
    const peer = await connections[1 - node].wait(frame => frame.type === 'message' && frame.messageId === accepted.messageId, 'independent encrypted peer delivery');
    assert.equal(peer.content, body); const opened = await keyRead(1 - node, key.epoch, key.key);
    try { assert.equal(digest(decrypt(opened, Buffer.from(peer.content.split(':')[3], 'base64'), messageAad(key))), digest(plain)); }
    finally { opened.fill(0); }
    for (const founder of founders) {
      const stored = (await history(founder, channel)).find(value => value.message_id === accepted.messageId); assert(stored);
      assert.equal(stored.content, body); assert(!stored.content.includes(plain));
      const raw = readRows(founder.url, 'SELECT content FROM messages WHERE message_id=?', [accepted.messageId])[0];
      assert(raw.content.startsWith('\x01ar1:')); assert(!raw.content.includes(plain));
    }
    return accepted;
  }
  async function grant(node, device = devices[node].certificate.device_id) {
    return (await api(node, '/api/uploads/grant', { method: 'POST', body: { device_id: device } })).token;
  }
  async function fetchFile(node, item, token, expected = 200) {
    const url = new URL(item.descriptor.url, founders[node].url); url.searchParams.set('t', token);
    const response = await fetch(url, { signal: AbortSignal.timeout(45_000) });
    if (expected !== 200) { assert([].concat(expected).includes(response.status), `Actual encrypted file refusal ${response.status}`); await response.arrayBuffer(); return; }
    if ([404, 409, 503].includes(response.status)) { await response.arrayBuffer(); return null; }
    assert.equal(response.status, 200); const bytes = Buffer.from(await response.arrayBuffer());
    assert.equal(digest(bytes), item.descriptor.ciphertextSha256); assert.equal(bytes.length, item.plain.length + 28);
    const aad = Buffer.from(JSON.stringify(['annex-attachment-v1', authority, channel, item.descriptor.id, item.descriptor.epoch, item.descriptor.keyId]));
    assert.equal(digest(decrypt(Buffer.from(item.descriptor.fileKeyHex, 'hex'), bytes, aad)), digest(item.plain));
    return bytes;
  }
  async function attachment(node, key) {
    const id = randomUUID(), fileKey = randomBytes(32), plain = Buffer.from(`confidential file ${randomUUID()}\n`);
    const name = `private-${randomUUID()}.txt`, aad = Buffer.from(JSON.stringify(['annex-attachment-v1', authority, channel, id, key.epoch, key.id]));
    const bytes = encrypt(fileKey, plain, aad), form = new FormData();
    for (const [field, value] of Object.entries({ attachment_id: id, epoch: String(key.epoch), key_id: key.id, category: 'file' })) form.append(field, value);
    form.append('file', new Blob([bytes], { type: 'application/octet-stream' }), `${id}.bin`);
    await api(node, `/api/channels/${channel}/upload-policy`);
    const response = await fetch(`${founders[node].url}/api/channels/${channel}/encrypted-upload`, {
      method: 'POST', headers: sessionHeaders(founders[node].token), body: form, signal: AbortSignal.timeout(30_000),
    });
    assert.equal(response.status, 200); const upload = await response.json();
    assert.equal(upload.upload_id, id); assert.equal(upload.ciphertext_sha256, digest(bytes)); assert.equal(upload.encrypted, true);
    const descriptor = { id, url: upload.url, name, contentType: 'text/plain', category: 'file', bytes: plain.length,
      plaintextSha256: digest(plain), ciphertextSha256: digest(bytes), fileKeyHex: fileKey.toString('hex'),
      epoch: key.epoch, keyId: key.id, authorityNamespace: authority, channelId: channel,
      originIdentity: upload.origin_identity, originatingServer: upload.originating_server };
    fileKey.fill(0);
    const privateBody = `annex.message.v1:${JSON.stringify({ text: `file marker ${randomUUID()}`, attachments: [descriptor] })}`;
    const sent = await message(node, privateBody, key, [id]);
    const item = { node, descriptor, plain, bytes, message: sent, privateBody, paths: [], grants: [] };
    for (let peer = 0; peer < 2; peer++) {
      const token = await grant(peer); item.grants[peer] = token;
      const actual = await until(() => fetchFile(peer, item, token), 'actual opaque binary transfer and independent decryption', 90_000);
      assert.equal(digest(actual), digest(bytes)); assert.equal(digest(readUploadBytes(founders[peer].url, id)), digest(bytes));
      const metadata = readRows(founders[peer].url, 'SELECT * FROM uploads WHERE upload_id=?', [id])[0]; assert(metadata);
      assert.equal(metadata.encrypted, 1); assert.equal(metadata.original_filename, 'encrypted.bin');
      assert.equal(metadata.content_type, 'application/octet-stream'); assert.equal(metadata.size_bytes, bytes.length);
      assert.equal(metadata.ciphertext_sha256, digest(bytes)); item.paths[peer] = metadata.storage_path;
      const raw = JSON.stringify(metadata); for (const secret of [name, plain.toString('utf8'), descriptor.fileKeyHex]) assert(!raw.includes(secret));
      const binding = readRows(founders[peer].url, 'SELECT descriptor_json FROM channel_authority_attachment_bindings WHERE attachment_id=?', [id])[0]; assert(binding);
      for (const secret of [name, plain.toString('utf8'), descriptor.fileKeyHex]) assert(!binding.descriptor_json.includes(secret));
      assert.equal(readRows(founders[peer].url, 'SELECT COUNT(*) AS n FROM message_uploads WHERE message_id=? AND upload_id=?', [sent.messageId, id])[0].n, 1);
    }
    attachments.push(item); emit('Actual opaque attachment decrypted at both peers; disk and signed metadata contain ciphertext/opaque IDs only',
      { attachmentId: id, messageId: sent.messageId, bytes: bytes.length, ciphertextSha256: digest(bytes) });
    return item;
  }
  try {
    const outsider = await register(hostFounder.url);
    for (let node = 0; node < 2; node++) devices[node] = await certify(node);
    authority = devices[0].context.server_identity;
    await createChannel(hostFounder, channel); await joinChannel(hostFounder, channel);
    await api(0, `/api/channels/${channel}/authority/initialize`, { method: 'POST', body: { expected_authority_identity: authority } });
    const peers = await api(1, '/api/federation/authority-peers'), peer = peers.peers.find(value => value.base_url === hostFounder.url); assert(peer);
    await api(1, `/api/federation/channels/${channel}/attach`, { method: 'POST', body: {
      peer_id: peer.peer_id, authority_identity: authority, device_id: devices[1].certificate.device_id,
    } });
    await append(0, { kind: 'genesis', join_policy: 'eligible_self_join', history_policy: 'all_members', certificate: devices[0].certificate });
    await append(1, { kind: 'join', certificate: devices[1].certificate });
    // A rejected operation retains its immutable intent ID at the origin.
    // Fault a distinct unused departure, not the successful Join's same ID.
    const invalid = signEvent(1, { kind: 'leave' }); invalid.signature_hex = '00'.repeat(64);
    await api(1, `/api/channels/${channel}/key-manifest`, { method: 'POST', body: invalid, expected: 403 });
    await verifyManifest(0); await verifyManifest(1);
    await request(outsider.url, `/api/channels/${channel}/member-keys`, { token: outsider.token, expected: 403 });
    const directory = await api(1, `/api/channels/${channel}/member-keys`); assert.equal(recipients(directory).length, 2);
    const invalidKey = randomBytes(32), invalidWrap = wraps(1, directory, invalidKey); invalidKey.fill(0);
    invalidWrap.authorization.signature_hex = '00'.repeat(64);
    await api(1, `/api/channels/${channel}/key-wraps`, { method: 'POST', body: invalidWrap, expected: 403 });
    const initial = await commit(1); // The remote HUMAN genuinely commits at the pinned authority.
    const staleKey = randomBytes(32);
    try { await api(0, `/api/channels/${channel}/key-wraps`, { method: 'POST', body: wraps(0, directory, staleKey), expected: 409 }); }
    finally { staleKey.fill(0); }
    emit('Genuine remote member-signed admission and epoch commit verified independently; forged signatures, outsider directory and stale CAS refused',
      { channel, authority, manifestHead: events.at(-1).event_hash, epoch: initial.epoch, keyId: initial.id });
    for (const founder of founders) { const connection = await socket(founder); connection.send({ type: 'subscribe', channelId: channel }); connections.push(connection); }
    for (let node = 0; node < 2; node++) await attachment(node, initial);
    const unboundGrant = await api(1, '/api/uploads/grant', { method: 'POST', body: {} });
    await fetchFile(1, attachments[0], unboundGrant.token, [403, 404]);
    await api(1, '/api/uploads/grant', { method: 'POST', body: { device_id: devices[0].certificate.device_id }, expected: 403 });
    const editId = randomUUID(), edited = `annex.message.v1:${JSON.stringify({ text: `edited-${randomUUID()}`, attachments: [attachments[1].descriptor] })}`;
    connections[1].send({ type: 'edit_message', channelId: channel, messageId: attachments[1].message.messageId,
      content: bodyFor(edited, initial), clientRequestId: editId });
    assert.equal((await connections[1].wait(frame => frame.clientRequestId === editId, 'encrypted remote edit')).type, 'message_edited');
    await connections[0].wait(frame => frame.type === 'message_edited' && frame.messageId === attachments[1].message.messageId, 'independent encrypted edit');
    for (let node = 0; node < 2; node++) {
      const edits = await api(node, `/api/channels/${channel}/messages/${attachments[1].message.messageId}/edits`);
      assert(edits.some(value => value.old_content === attachments[1].message.content));
      assert.equal(digest(await fetchFile(node, attachments[1], attachments[1].grants[node])), digest(attachments[1].bytes));
    }
    const wrongAad = Buffer.from(JSON.stringify(['annex-message-v2', devices[1].context.server_identity, channel, initial.epoch, initial.id]));
    assert.throws(() => decrypt(initial.key, Buffer.from(attachments[0].message.content.split(':')[3], 'base64'), wrongAad));
    emit('Remote encrypted edit preserves ciphertext audit/file association; an unrelated authority namespace cannot decrypt');

    await setPeerOnline(hostFounder.url, false); offline = hostFounder.url;
    await fetchFile(1, attachments[0], attachments[0].grants[1], 503);
    await fetchFile(1, attachments[1], attachments[1].grants[1], 503);
    await setPeerOnline(hostFounder.url, true); offline = null;
    connections[0] = await socket(hostFounder); connections[0].send({ type: 'subscribe', channelId: channel });
    for (const item of attachments) await until(() => fetchFile(1, item, item.grants[1]), 'fresh authority access after actual same-disk restart', 90_000);
    await verifyManifest(0); await verifyManifest(1); (await keyRead(1, initial.epoch, initial.key)).fill(0);
    emit('Both origin-owned and replica encrypted bytes refuse reads during actual authority outage and recover after same-disk restart');

    const oldDevice = devices[1], oldGrant = await grant(1);
    devices[1] = await certify(1, oldDevice.certificate);
    await append(1, { kind: 'recover_device', certificate: devices[1].certificate });
    // Recovery does not erase prior sealed history entitlements. A globally
    // valid old certificate can retain old wraps, but gains no new epoch or
    // current attachment authority from them.
    const preservedHistory = await api(1, `/api/channels/${channel}/key-wraps?device_id=${oldDevice.certificate.device_id}`);
    const oldWrap = preservedHistory.wraps.find(value => value.epoch === initial.epoch); assert(oldWrap);
    const historicalFields = JSON.parse(open(Buffer.from(oldWrap.wrapped_key_b64, 'base64'), oldDevice.device.privateKey).toString('utf8'));
    sameSecret(Buffer.from(historicalFields[7], 'base64'), initial.key);
    await fetchFile(1, attachments[0], oldGrant, [403, 404]);
    // Newly certified material has no historical file entitlement before wraps.
    const replacementGrant = await grant(1);
    await fetchFile(1, attachments[0], replacementGrant, [403, 404]);
    const rotated = await commit(0);
    const excludedOldDevice = await api(1, `/api/channels/${channel}/key-wraps?device_id=${oldDevice.certificate.device_id}`);
    assert(!excludedOldDevice.wraps.some(value => value.epoch === rotated.epoch));
    const oldDirectory = await api(0, `/api/channels/${channel}/member-keys`);
    const oldEpoch = oldDirectory.historical_epochs.find(value => value.epoch === initial.epoch && value.key_id === initial.id);
    assert(oldEpoch?.missing_devices.some(value => value.device_id === devices[1].certificate.device_id));
    const historyWraps = wraps(0, oldDirectory, initial.key, 'admit_history_devices', initial.epoch, oldEpoch.missing_devices);
    await api(0, `/api/channels/${channel}/key-wraps`, { method: 'POST', body: historyWraps });
    const recoveredKey = await keyRead(1, initial.epoch, initial.key);
    try {
      const oldMessages = await history(vmFounder, channel);
      for (const item of attachments) {
        const stored = oldMessages.find(value => value.message_id === item.message.messageId); assert(stored);
        const plain = decrypt(recoveredKey, Buffer.from(stored.content.split(':')[3], 'base64'), messageAad(initial)).toString('utf8');
        assert(plain.startsWith('annex.message.v1:'));
        const recovered = JSON.parse(plain.slice('annex.message.v1:'.length)).attachments.find(value => value.id === item.descriptor.id);
        assert.equal(digest(json(recovered)), digest(json(item.descriptor)), 'New device derives the actual historical file key from received ciphertext');
      }
    } finally { recoveredKey.fill(0); }
    for (const item of attachments) await until(() => fetchFile(1, item, replacementGrant), 'explicit historical entitlement after actual device recovery');
    connections[1].ws.close(); connections[1] = await socket(vmFounder); connections[1].send({ type: 'subscribe', channelId: channel });
    await message(1, `post-recovery-${randomUUID()}`, rotated);
    emit('Actual new-device recovery preserves prior sealed history but revokes current old-device file authority, excludes it from new keys and requires signed history admission for the replacement',
      { previousDevice: oldDevice.certificate.device_id, replacementDevice: devices[1].certificate.device_id, epoch: rotated.epoch });

    await setPeerOnline(vmFounder.url, false); offline = vmFounder.url;
    await append(0, { kind: 'remove_member', pseudonym_id: vmFounder.pseudonym }, [0]);
    const finalEpoch = await commit(0, [0]);
    await setPeerOnline(vmFounder.url, true); offline = null;
    await api(1, `/api/channels/${channel}/messages?limit=100`, { expected: 403 });
    await api(1, `/api/channels/${channel}/key-wraps?device_id=${devices[1].certificate.device_id}`, { expected: 403 });
    for (const item of attachments) await fetchFile(1, item, replacementGrant, [403, 404]);
    assert.equal(readRows(hostFounder.url, 'SELECT COUNT(*) AS n FROM channel_device_key_wraps WHERE channel_id=? AND epoch=? AND recipient_pseudonym_id=?',
      [channel, finalEpoch.epoch, vmFounder.pseudonym])[0].n, 0);
    emit('Signed member removal during real peer outage denies fresh history/wraps/files on reconnect and excludes that member from the next epoch');

    await api(0, `/api/channels/${channel}`, { method: 'DELETE' });
    for (const item of attachments) for (let node = 0; node < 2; node++) {
      await until(() => readRows(founders[node].url, 'SELECT COUNT(*) AS n FROM uploads WHERE upload_id=?', [item.descriptor.id])[0].n === 0,
        'ordered encrypted-channel deletion removes actual upload metadata', 90_000);
      await until(() => readUploadBytes(founders[node].url, item.descriptor.id, item.paths[node]) === null,
        'actual opaque file deletion on both peers', 60_000);
      await fetchFile(node, item, node === 0 ? item.grants[0] : replacementGrant, [403, 404]);
    }
    emit('Ordered encrypted-channel deletion removes both original and replica disk bytes and refuses saved grants');
    return { complete: true, channelId: channel, actualDeviceCertificates: 3, epochs: keys.size,
      scope: 'Real HTTP or configured data-channel protocol and independent crypto; browser encrypted UI is a separate lane' };
  } finally {
    for (const connection of connections) connection.ws.close();
    for (const key of keys.values()) key.fill(0);
    if (offline) await setPeerOnline(offline, true);
  }
}
