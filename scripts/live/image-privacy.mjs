// Actual multipart HTTP acceptance against a fresh running Annex process.
// Identities authenticate with the real membership circuit. SQLite is read-only.
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync, copyFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { register, request, digest, createChannel, joinChannel } from './protocol.mjs';

const fixtureDir = fileURLToPath(new URL('../../crates/annex-server/tests/fixtures/image-privacy/', import.meta.url));
const mime = { jpg: 'image/jpeg', png: 'image/png', gif: 'image/gif', webp: 'image/webp' };
const privateMarker = Buffer.from('ANNEX_PRIVATE_METADATA_LOCATION_AUTHOR_12345');
const files = directory => existsSync(directory)
  ? readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory()
    ? files(join(directory, entry.name)) : [join(directory, entry.name)]) : [];

async function multipart(identity, route, name, bytes, expected = 200) {
  const body = new FormData();
  // Deliberately misleading declared type: only the actual bytes are authoritative.
  body.append('file', new Blob([bytes], { type: 'application/octet-stream' }), name);
  const response = await fetch(new URL(route, identity.url), {
    method: 'POST', headers: { authorization: `Bearer ${identity.token}` }, body,
    signal: AbortSignal.timeout(30_000),
  });
  const text = await response.text();
  assert.equal(response.status, expected, `Multipart ${route}: HTTP ${response.status}${response.ok ? '' : ` ${text}`}`);
  return response.ok ? JSON.parse(text) : null;
}

async function download(base, path, grant, expected = 200) {
  const url = new URL(path, base);
  if (grant) url.searchParams.set('t', grant);
  const response = await fetch(url, { signal: AbortSignal.timeout(15_000) });
  assert.equal(response.status, expected, `Attachment fetch: HTTP ${response.status}`);
  return { bytes: Buffer.from(await response.arrayBuffer()), type: response.headers.get('content-type') };
}

async function main(session) {
  assert.equal(digest(readFileSync(session.binary)).toLowerCase(), session.binary_sha256.toLowerCase());
  const output = join(session.data, 'image-privacy-http');
  assert(!existsSync(output), 'Use a fresh session; never replace an earlier acceptance result');
  mkdirSync(output);
  const definitions = join(output, 'definitions'); mkdirSync(definitions);
  const definitionsManifest = [];
  for (const name of ['image-privacy.mjs', 'protocol.mjs']) {
    const source = fileURLToPath(new URL(name, import.meta.url));
    copyFileSync(source, join(definitions, name));
    definitionsManifest.push({ path: name, sha256: digest(readFileSync(source)) });
  }
  const receipts = [];
  let complete = false;
  const record = (action, details = {}) => {
    receipts.push({ at: new Date().toISOString(), action, ...details });
    console.log(`[image-privacy-live] ${action}`);
  };
  let db;
  try {
    const founder = await register(session.url);
    const member = await register(session.url);
    const outsider = await register(session.url);
    const channel = `image-privacy-${Date.now()}`;
    await createChannel(founder, channel, 'Text', 'Local');
    await joinChannel(founder, channel); await joinChannel(member, channel);
    const grant = (await request(session.url, '/api/uploads/grant', { method: 'POST', body: {}, token: member.token })).token;
    const outsiderGrant = (await request(session.url, '/api/uploads/grant', { method: 'POST', body: {}, token: outsider.token })).token;
    db = new DatabaseSync(join(session.data, 'annex.db'), { readOnly: true });
    const cleaned = JSON.parse(readFileSync(join(fixtureDir, 'decoded-cleaned.json'), 'utf8'));
    assert.equal(cleaned.complete, true); assert.equal(cleaned.results.length, 5);
    const chatPaths = [];
    let priorBrand;
    for (const routeKind of ['server-image', 'chat-image']) {
      const destination = join(output, routeKind); mkdirSync(destination);
      for (const fixture of cleaned.results) {
        const bytes = readFileSync(join(fixtureDir, fixture.file));
        assert(bytes.includes(privateMarker), `${fixture.file} must exercise real private metadata`);
        const type = mime[fixture.file.split('.').pop()];
        const brand = routeKind === 'server-image';
        const uploaded = await multipart(brand ? founder : member,
          brand ? '/api/admin/server/image' : `/api/channels/${channel}/upload`, fixture.file, bytes);
        const served = await download(session.url, uploaded.url, brand ? null : grant);
        assert.equal(served.type, type); assert.equal(digest(served.bytes), fixture.sha256);
        assert(!served.bytes.includes(privateMarker));
        assert(served.bytes.length < bytes.length, 'Source fixture has removable private metadata');
        const row = db.prepare('SELECT content_type,size_bytes,ciphertext_sha256,uploader_pseudonym,channel_id FROM uploads WHERE upload_id=?').get(uploaded.upload_id);
        assert.equal(row.content_type, type); assert.equal(row.size_bytes, served.bytes.length);
        assert.equal(row.ciphertext_sha256, fixture.sha256);
        assert.equal(row.uploader_pseudonym, (brand ? founder : member).pseudonym);
        if (!brand) {
          assert.equal(row.channel_id, channel); assert.equal(uploaded.category, 'image');
          assert.equal(uploaded.metadata_stripped_bytes, bytes.length - served.bytes.length);
          assert.equal(uploaded.ciphertext_sha256, fixture.sha256);
          await download(session.url, uploaded.url, null, 401);
          await download(session.url, uploaded.url, outsiderGrant, 404);
          chatPaths.push(uploaded.url);
        } else {
          if (priorBrand) await download(session.url, priorBrand, null, 404);
          priorBrand = uploaded.url;
        }
        writeFileSync(join(destination, fixture.file), served.bytes);
        record(`${routeKind}: real ${fixture.file} uploaded, stored and fetched with private metadata removed`, {
          source_sha256: digest(bytes), fetched_sha256: digest(served.bytes), content_type: type,
          encoded_size: served.bytes.length, expected_decoded_frames: fixture.frames.length,
        });
      }
    }
    const valid = readFileSync(join(fixtureDir, 'progressive.jpg'));
    await multipart(member, '/api/admin/server/image', 'member.jpg', valid, 403);
    await multipart(outsider, `/api/channels/${channel}/upload`, 'outsider.jpg', valid, 403);
    record('Nonmoderator branding and nonmember chat uploads refused; all chat URLs require a current member grant; retired branding is unavailable');
    const rowsBefore = db.prepare('SELECT upload_id FROM uploads ORDER BY upload_id').all();
    const filesBefore = new Set(files(join(session.data, 'uploads')));
    for (const fixture of cleaned.results) {
      const bytes = readFileSync(join(fixtureDir, fixture.file));
      const truncated = bytes.subarray(0, bytes.length - 3);
      await multipart(founder, '/api/admin/server/image', fixture.file, truncated, 400);
      await multipart(member, `/api/channels/${channel}/upload`, fixture.file, truncated, 400);
    }
    assert.deepEqual(db.prepare('SELECT upload_id FROM uploads ORDER BY upload_id').all(), rowsBefore);
    assert.equal(files(join(session.data, 'uploads')).filter(file => !filesBefore.has(file)).length, 0);
    record('Ten malformed-container HTTP uploads refused before creating any upload row or file');
    await request(session.url, `/api/channels/${channel}/leave`, { method: 'POST', token: member.token, body: {} });
    for (const path of chatPaths) await download(session.url, path, grant, 404);
    record('Leaving the channel immediately revokes all five existing chat attachment URLs with their previously valid grant');
    complete = true;
  } catch (error) {
    record('FAILED', { error: error.message }); process.exitCode = 1;
  } finally {
    db?.close();
    writeFileSync(join(output, 'evidence.json'), JSON.stringify({
      complete, binary_sha256: session.binary_sha256, definitions: definitionsManifest,
      boundary: 'Real native HTTP, actual membership proofs, read-only persisted bytes. Decoded frame checks are a separate required independent verification of these downloaded files.',
      receipts,
    }, null, 2));
  }
  return complete;
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const session = JSON.parse(readFileSync(process.argv[2], 'utf8').replace(/^\uFEFF/, ''));
  process.exit(await main(session) ? 0 : 1);
}
