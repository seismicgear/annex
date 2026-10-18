import { expect, test, type Locator } from '@playwright/test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { actor, auth, Companion, get, type Actor, type RecordFile } from './federation-ui-helpers';
import type { Channel, Message } from '../../src/types';
import type { AuthorityPeer, ChannelAuthority } from '../../src/api/channel-authority';

const sha256 = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
interface Upload { upload_id: string; url: string; category: string; size: number; encrypted: boolean; epoch: number | null; key_id: string | null; ciphertext_sha256: string }
interface DiskRow { upload_id: string; purpose: string; encrypted: number; epoch: number | null; key_id: string | null; size_bytes: number; ciphertext_sha256: string; lifecycle: string; storage_path: string; staging_path: string | null }
interface Sent { upload: Upload; message: Message; marker: string; expectedHash: string }

/** Independent observation of the two disposable stores, never fixture writes. */
function diskEvidence(recordPath: string, expectedUrl: string) {
  const record = JSON.parse(readFileSync(recordPath, 'utf8').replace(/^\uFEFF/, '')) as RecordFile & { data: string; binary: string };
  assert.equal(new URL(record.url).origin, new URL(expectedUrl).origin);
  assert.equal(sha256(readFileSync(record.binary)), record.binary_sha256.toLowerCase());
  const root = realpathSync(join(dirname(recordPath), 'sessions')), data = realpathSync(record.data);
  const distance = relative(root, data);
  assert(distance && !distance.startsWith('..') && !isAbsolute(distance), 'Inspect only the captured disposable server data');
  const uploads = join(data, 'uploads');
  function location(path: string) {
    assert(path && !isAbsolute(path) && !path.split(/[\\/]/).includes('..'));
    const result = join(uploads, path);
    assert(!relative(uploads, result).startsWith('..'));
    return result;
  }
  const db = new DatabaseSync(join(data, 'annex.db'), { readOnly: true });
  return {
    row(id: string): DiskRow | undefined {
      return db.prepare('SELECT upload_id,purpose,encrypted,epoch,key_id,size_bytes,ciphertext_sha256,lifecycle,storage_path,staging_path FROM uploads WHERE upload_id=?').get(id) as unknown as DiskRow | undefined;
    },
    stored(row: DiskRow) { return sha256(readFileSync(location(row.storage_path))); },
    linked(message: string, upload: string) { return Number(db.prepare('SELECT count(*) n FROM message_uploads WHERE message_id=? AND upload_id=?').get(message, upload)!.n); },
    removed(row: DiskRow) {
      return !this.row(row.upload_id) && !existsSync(location(row.storage_path)) && (!row.staging_path || !existsSync(location(row.staging_path)))
        && Number(db.prepare('SELECT count(*) n FROM upload_deletion_queue WHERE relative_path=? OR relative_path=?').get(row.storage_path, row.staging_path ?? '')!.n) === 0;
    },
    close() { db.close(); },
  };
}
function row(who: Actor, channelName: string) {
  return who.page.locator('.channel-item').filter({ has: who.page.locator('.channel-name', { hasText: new RegExp(`^${channelName}$`) }) });
}
function attachment(who: Actor, id: string) { return who.page.locator(`.federated-attachment[data-attachment-id="${id}"]`); }
function bubble(who: Actor, marker: string) { return who.page.locator('.message').filter({ has: who.page.locator('.message-content', { hasText: marker }) }); }
async function select(who: Actor, channelName: string) {
  await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
  await row(who, channelName).locator('.channel-select').click();
  await expect(row(who, channelName)).toHaveClass(/active/);
  await expect(who.page.getByPlaceholder('Type a message...')).toBeEnabled();
}
async function message(who: Actor, channel: string, marker: string) {
  let observed: Message | undefined;
  await expect.poll(async () => {
    const matches = (await get<Message[]>(who, `/api/channels/${channel}/messages?limit=100`)).filter(item => item.content.startsWith(marker));
    expect(matches.length).toBeLessThanOrEqual(1); observed = matches[0]; return Boolean(observed);
  }).toBe(true);
  return observed!;
}
async function send(who: Actor, channel: string, marker: string, file: { name: string; mimeType: string; buffer: Buffer }, expectedHash: string): Promise<Sent> {
  await who.page.locator('.message-input input[type="file"]').setInputFiles(file);
  await expect(who.page.locator('.image-preview-name')).toContainText(file.name);
  await who.page.getByPlaceholder('Add a caption (optional)...').fill(marker);
  const response = who.page.waitForResponse(value => value.request().method() === 'POST' && new URL(value.url()).pathname === `/api/channels/${channel}/upload`);
  await who.page.getByRole('button', { name: 'Send', exact: true }).click();
  const actual = await response; expect(actual.status()).toBe(200);
  const upload = await actual.json() as Upload;
  expect(upload.encrypted).toBe(false); expect(upload.epoch).toBeNull(); expect(upload.key_id).toBeNull();
  expect(upload.ciphertext_sha256).toBe(expectedHash);
  const observed = await message(who, channel, marker);
  expect(observed.content).toBe(`${marker}\n${upload.url}`);
  await expect(bubble(who, marker).locator('.reply-btn')).toBeVisible();
  return { upload, message: observed, marker, expectedHash };
}
async function download(who: Actor, part: Locator, expectedHash: string) {
  const link = part.getByRole('link', { name: /^Download / });
  await expect(link).toHaveAttribute('href', /^blob:/);
  const received = who.page.waitForEvent('download'); await link.click();
  const transfer = await received; expect(await transfer.failure()).toBeNull();
  const path = await transfer.path(); expect(path).toBeTruthy();
  try { const bytes = await readFile(path!); expect(sha256(bytes)).toBe(expectedHash); return bytes.length; }
  finally { await transfer.delete(); }
}
async function remove(who: Actor, sent: Sent) {
  // The product offers self-deletion for sixty seconds: each destructive
  // assertion creates its own fresh real message instead of bypassing the UI.
  const target = bubble(who, sent.marker);
  await target.locator('.delete-btn').click(); await target.locator('.delete-btn').click();
  await expect(target).toHaveCount(0);
}

test('ordinary federated attachments preserve bytes, enforce current membership, and recover across actual server outages', async ({ browser }) => {
  const files = [process.env.ANNEX_FEDERATION_UI_HOST_SESSION, process.env.ANNEX_FEDERATION_UI_PEER_SESSION];
  expect(files.every(Boolean), 'Supply a separate fresh captured pair; other federation suites mutate their own agreements').toBe(true);
  const records = await Promise.all(files.map(async file => JSON.parse((await readFile(file!, 'utf8')).replace(/^\uFEFF/, '')) as RecordFile));
  const people = [await actor(browser, records[0].url), await actor(browser, records[1].url)];
  const [owner, peer] = people, companion = new Companion(files as string[]);
  const peerNetwork = await peer.context.newCDPSession(peer.page); await peerNetwork.send('Network.enable');
  const disks = files.map((file, index) => diskEvidence(file!, records[index].url));
  const privateReadUrls = people.map(() => new Map<string, string>());
  const statuses: Array<{ server: number; id: string; status: number }> = [], hints: Array<Record<string, unknown>> = [];
  const grantDevices = people.map(() => new Set<string>()), receipts: Record<string, unknown> = {};
  const nonce = `ordinary-files-${Date.now()}`, name = `${nonce}-room`;
  let channel = '';
  for (const [index, who] of people.entries()) {
    who.page.on('request', request => {
      const url = new URL(request.url());
      const id = /^\/uploads\/chat\/(?:images|videos|files)\/([a-f0-9-]{36})\./.exec(url.pathname)?.[1];
      if (id && url.origin === new URL(who.url).origin && url.searchParams.has('t')) privateReadUrls[index].set(id, url.href);
      if (request.method() === 'POST' && url.pathname === '/api/uploads/grant') {
        const body = request.postDataJSON() as { device_id?: unknown } | null;
        if (typeof body?.device_id === 'string' && /^[a-f0-9]{64}$/.test(body.device_id)) grantDevices[index].add(body.device_id);
      }
    });
    who.page.on('response', response => {
      const id = /^\/uploads\/chat\/(?:images|videos|files)\/([a-f0-9-]{36})\./.exec(new URL(response.url()).pathname)?.[1];
      if (id) statuses.push({ server: index, id, status: response.status() });
    });
    who.page.on('websocket', socket => socket.on('framereceived', ({ payload }) => {
      try { const frame = JSON.parse(typeof payload === 'string' ? payload : payload.toString()) as Record<string, unknown>;
        if (frame.type === 'attachment_transfer_changed') hints.push({ server: index, attachmentId: frame.attachmentId, status: frame.status, transferredBytes: frame.transferredBytes, totalBytes: frame.totalBytes });
      } catch { /* Binary media and other frames carry no attachment receipt. */ }
    }));
  }
  try {
    await test.step('Both genuine founders initialize and explicitly pin an ordinary federated channel', async () => {
      // Attach passive socket observers before these real reconnects so hints
      // from the ordinary browser session are recorded, including early ones.
      for (const who of people) { await who.page.reload(); await expect(who.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 }); }
      for (const who of people) await expect(who.page.locator('.admin-menu-btn')).toBeVisible();
      const setup = await companion.command('start', { headers: await Promise.all(people.map(auth)) });
      const authority = (setup.authorityIdentities as string[])[0]; expect(authority).toMatch(/^annex-server:[a-f0-9]{64}$/);
      await owner.page.getByRole('button', { name: 'Chat', exact: true }).click();
      await owner.page.locator('.create-channel-btn').click();
      const create = owner.page.getByRole('dialog', { name: 'Create Channel', exact: true });
      await create.getByPlaceholder('general').fill(name); await create.getByRole('combobox').selectOption('Text');
      await create.getByRole('checkbox', { name: /^Federated/ }).check();
      await create.getByRole('button', { name: 'Create', exact: true }).click(); await expect(create).toHaveCount(0);
      const actual = (await get<Channel[]>(owner, '/api/channels')).find(value => value.name === name)!;
      expect(actual.federation_scope).toBe('Federated'); channel = actual.channel_id;
      await select(owner, name); await row(owner, name).getByTitle('Channel authority', { exact: true }).click();
      const initialize = owner.page.getByRole('dialog', { name: 'Channel authority', exact: true });
      await expect(initialize).toContainText(authority);
      await initialize.getByRole('button', { name: 'Initialize channel authority', exact: true }).click();
      await initialize.getByRole('region', { name: 'Confirm channel initialization', exact: true }).getByRole('button', { name: 'Confirm initialization', exact: true }).click();
      await expect(initialize.getByRole('status')).toContainText('This server accepts changes');
      await initialize.getByRole('button', { name: 'Done', exact: true }).click();
      await peer.page.getByRole('button', { name: 'Chat', exact: true }).click();
      await peer.page.getByRole('button', { name: 'Federated channels', exact: true }).click();
      const catalog = peer.page.getByRole('dialog', { name: 'Federated channels', exact: true });
      const partner = (await get<{ peers: AuthorityPeer[] }>(peer, '/api/federation/authority-peers')).peers.find(value => value.base_url === owner.url)!;
      expect(partner.authority_identity).toBe(authority);
      await catalog.getByRole('combobox', { name: 'Partner server', exact: true }).selectOption(String(partner.peer_id));
      await catalog.getByRole('button', { name: 'Load partner channels', exact: true }).click();
      await catalog.locator(`[data-federated-channel="${channel}"]`).getByRole('button', { name: 'Join channel', exact: true }).click();
      const confirmation = catalog.getByRole('region', { name: 'Confirm federated channel', exact: true });
      await expect(confirmation).toContainText(authority);
      await confirmation.getByLabel('I verified this partner fingerprint', { exact: true }).check();
      await confirmation.getByRole('button', { name: 'Confirm joining', exact: true }).click(); await expect(catalog).toHaveCount(0);
      await select(peer, name);
      expect((await get<ChannelAuthority>(peer, `/api/channels/${channel}/authority`)).authority_identity).toBe(authority);
      receipts.setup = { channel, authority, binaries: records.map(value => value.binary_sha256) };
    });

    const fixture = fileURLToPath(new URL('../../../crates/annex-server/tests/fixtures/image-privacy/animated.png', import.meta.url));
    const imageBytes = await readFile(fixture);
    const corpus = JSON.parse(await readFile(fileURLToPath(new URL('../../../crates/annex-server/tests/fixtures/image-privacy/decoded-cleaned.json', import.meta.url)), 'utf8')) as { complete: boolean; results: Array<{ file: string; sha256: string }> };
    expect(corpus.complete).toBe(true); const imageHash = corpus.results.find(value => value.file === 'animated.png')!.sha256;
    const transferred: Sent[] = [];
    await test.step('Small plaintext files and real sanitized images travel both ways, decode, and download as the exact accepted bytes', async () => {
      for (const source of [0, 1]) for (const kind of ['file', 'image']) {
        const data = kind === 'file' ? Buffer.from(`node${source}\n`) : imageBytes;
        if (kind === 'file') expect(data.length).toBeLessThan(28);
        const sent = await send(people[source], channel, `${nonce}-${source}-${kind}`,
          { name: kind === 'file' ? 'small.txt' : 'animated.png', mimeType: kind === 'file' ? 'text/plain' : 'image/png', buffer: data }, kind === 'file' ? sha256(data) : imageHash);
        transferred.push(sent);
        for (const [index, who] of people.entries()) {
          expect((await message(who, channel, sent.marker)).message_id).toBe(sent.message.message_id);
          const part = attachment(who, sent.upload.upload_id);
          expect(await download(who, part, sent.expectedHash)).toBe(sent.upload.size);
          if (kind === 'image') {
            await expect.poll(() => part.getByAltText('Uploaded image', { exact: true }).evaluate(image => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
            await part.getByRole('button', { name: 'View image full size', exact: true }).click();
            const preview = who.page.getByRole('dialog', { name: 'Full size attachment', exact: true });
            await expect.poll(() => preview.getByAltText('Full size', { exact: true }).evaluate(image => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
            await preview.getByRole('button', { name: 'Close image', exact: true }).click(); await expect(preview).toHaveCount(0);
          }
          const stored = disks[index].row(sent.upload.upload_id)!;
          expect(stored).toBeDefined(); expect(stored.encrypted).toBe(0); expect(stored.epoch).toBeNull(); expect(stored.key_id).toBeNull();
          expect(stored.purpose).toBe(index === source ? 'chat' : 'federated_chat');
          expect(stored.size_bytes).toBe(sent.upload.size); expect(disks[index].stored(stored)).toBe(sent.expectedHash);
          expect(disks[index].linked(sent.message.message_id, sent.upload.upload_id)).toBe(1);
        }
      }
      for (const devices of grantDevices) expect(devices.size, 'Each live browser must issue its own real device-bound read grant').toBeGreaterThan(0);
      receipts.bidirectional = transferred.map(value => ({ id: value.upload.upload_id, messageId: value.message.message_id, size: value.upload.size, sha256: value.expectedHash, category: value.upload.category }));
    });

    await test.step('An actual authority outage refuses cached replica reads and Retry; recovery restores the exact bytes', async () => {
      const sent = transferred[0], part = attachment(peer, sent.upload.upload_id);
      // A real browser transport failure exposes the ordinary Retry control.
      // This is deliberately not described as a paused background transfer.
      await select(peer, 'General');
      await peerNetwork.send('Network.setBlockedURLs', { urls: [`${peer.url}${sent.upload.url}*`] });
      await select(peer, name);
      await expect(part.getByRole('alert')).toBeVisible(); await expect(part.getByRole('link')).toHaveCount(0);
      await companion.command('offline', { peer: 0 });
      await peerNetwork.send('Network.setBlockedURLs', { urls: [] });
      const held = privateReadUrls[1].get(sent.upload.upload_id); expect(held).toBeTruthy();
      expect((await peer.context.request.get(held!)).status()).toBe(503);
      const retryPath = `/api/channels/${channel}/uploads/${sent.upload.upload_id}/retry`;
      const refused = peer.page.waitForResponse(value => value.request().method() === 'POST' && new URL(value.url()).pathname === retryPath);
      await part.getByRole('button', { name: 'Retry attachment', exact: true }).click(); expect((await refused).status()).toBe(503);
      await expect(part.getByRole('alert')).toBeVisible();
      await companion.command('online', { peer: 0 });
      const retried = peer.page.waitForResponse(value => value.request().method() === 'POST' && new URL(value.url()).pathname === retryPath);
      await part.getByRole('button', { name: 'Retry attachment', exact: true }).click(); expect((await retried).status()).toBe(200);
      expect(await download(peer, part, sent.expectedHash)).toBe(sent.upload.size);
      receipts.outage = { id: sent.upload.upload_id, failedRetryStatus: 503, acceptedRetryStatus: 200, actualReplicaBytesRetained: true };
    });

    await test.step('Leaving withdraws authority access even from a saved unexpired grant; explicit rejoin restores a fresh read', async () => {
      const sent = transferred[0];
      const held = privateReadUrls[1].get(sent.upload.upload_id); expect(held).toBeTruthy();
      const leaving = peer.page.waitForResponse(value => value.request().method() === 'POST' && new URL(value.url()).pathname === `/api/channels/${channel}/leave`);
      await row(peer, name).locator('.leave-btn').click(); expect((await leaving).status()).toBe(200);
      await expect(row(peer, name).locator('.join-btn')).toBeVisible(); await expect(peer.page.locator('.message-input')).toHaveCount(0);
      const denied = await peer.context.request.get(held!); expect([403, 404]).toContain(denied.status());
      const joined = peer.page.waitForResponse(value => value.request().method() === 'POST' && new URL(value.url()).pathname === `/api/channels/${channel}/join`);
      await row(peer, name).locator('.join-btn').click(); expect((await joined).status()).toBe(200); await select(peer, name);
      expect(await download(peer, attachment(peer, sent.upload.upload_id), sent.expectedHash)).toBe(sent.upload.size);
      receipts.withdrawal = { id: sent.upload.upload_id, oldGrantDenied: denied.status(), explicitRejoin: true };
    });

    await test.step('A fresh replica-authored deletion removes its origin and replica files, associations and download access', async () => {
      const bytes = Buffer.from('remove\n');
      const sent = await send(peer, channel, `${nonce}-delete-from-replica`, { name: 'remove.txt', mimeType: 'text/plain', buffer: bytes }, sha256(bytes));
      const stored: DiskRow[] = [];
      for (const [index, who] of people.entries()) {
        await download(who, attachment(who, sent.upload.upload_id), sent.expectedHash); stored.push(disks[index].row(sent.upload.upload_id)!);
      }
      const urls = privateReadUrls.map(map => map.get(sent.upload.upload_id)!); expect(urls.every(Boolean)).toBe(true);
      await remove(peer, sent);
      for (const [index, who] of people.entries()) {
        await expect(bubble(who, sent.marker)).toHaveCount(0);
        await expect.poll(() => disks[index].removed(stored[index]), { timeout: 45_000 }).toBe(true);
        expect(disks[index].linked(sent.message.message_id, sent.upload.upload_id)).toBe(0);
        expect([403, 404]).toContain((await who.context.request.get(urls[index])).status());
      }
      receipts.deletion = { id: sent.upload.upload_id, messageId: sent.message.message_id, bothPhysicalFilesRemoved: true };
    });

    await test.step('A stopped peer catches up a new accepted file and an ordered deletion after its real restart', async () => {
      const goneBytes = Buffer.from('gone\n');
      const gone = await send(owner, channel, `${nonce}-offline-delete`, { name: 'gone.txt', mimeType: 'text/plain', buffer: goneBytes }, sha256(goneBytes));
      await download(peer, attachment(peer, gone.upload.upload_id), gone.expectedHash);
      const removedRows = disks.map(disk => disk.row(gone.upload.upload_id)!);
      await companion.command('offline', { peer: 1 });
      await remove(owner, gone);
      const bytes = Buffer.from('catchup\n');
      const caught = await send(owner, channel, `${nonce}-offline-created`, { name: 'catchup.txt', mimeType: 'text/plain', buffer: bytes }, sha256(bytes));
      await companion.command('online', { peer: 1 });
      await expect(peer.page.getByPlaceholder('Type a message...')).toBeEnabled({ timeout: 60_000 });
      expect((await message(peer, channel, caught.marker)).message_id).toBe(caught.message.message_id);
      await download(peer, attachment(peer, caught.upload.upload_id), caught.expectedHash);
      await expect(bubble(peer, gone.marker)).toHaveCount(0);
      for (const [index, disk] of disks.entries()) await expect.poll(() => disk.removed(removedRows[index]), { timeout: 45_000 }).toBe(true);
      receipts.restart = { newUploadId: caught.upload.upload_id, removedUploadId: gone.upload.upload_id, exactlyOneAcceptedMessage: true, removedFilesAbsent: true };
    });
    for (const who of people) expect(who.errors).toEqual([]);
    await test.info().attach('ordinary-federated-attachments', { contentType: 'application/json', body: Buffer.from(JSON.stringify({ complete: true, receipts, observedReadStatuses: statuses, observedTransferHints: hints })) });
  } finally {
    // Grants, raw auth headers and private download URLs never enter artifacts.
    for (const values of privateReadUrls) values.clear();
    await peerNetwork.send('Network.setBlockedURLs', { urls: [] }); await peerNetwork.detach();
    await companion.close(); for (const disk of disks) disk.close(); for (const who of people) await who.context.close();
  }
});
