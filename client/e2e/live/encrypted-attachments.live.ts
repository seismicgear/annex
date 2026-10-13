import { expect, test, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { attachmentEvidence, inspectUpload, sha256, type UploadReceipt, type UploadRow } from './attachment-evidence';
import { attachmentProxy } from './attachment-proxy';
import { baseURL, publicIdentities, read, reload, row, select } from './operator-helpers';
import { bubble, createText, encryptionActor, linkDevice, ready, signedEvidence, trust, type EncryptionActor } from './encryption-helpers';
import type { Message } from '../../src/types';

const marker = 'ANNEX_PRIVATE_METADATA_LOCATION_AUTHOR_12345';
const imageDirectory = new URL('../../../crates/annex-server/tests/fixtures/image-privacy/', import.meta.url);
type Fixture = { name: string; mimeType: string; buffer: Buffer; category: 'image' | 'video' | 'file'; expectedSha256: string };
type Published = { receipt: UploadReceipt; stored: UploadRow; messageId: string; fixture: Fixture; caption: string };
const attachment = (actor: EncryptionActor, id: string) => actor.page.locator(`.encrypted-attachment[data-attachment-id="${id}"]`);

/** Create a real WebM with Chromium's encoder. The product receives the resulting
 * File bytes normally; no media element or application state is substituted. */
async function recordedVideo(page: Page): Promise<Fixture> {
  const encoded = await page.evaluate(async () => {
    const canvas = document.createElement('canvas'); canvas.width = 160; canvas.height = 96;
    const context = canvas.getContext('2d')!;
    const stream = canvas.captureStream(15);
    const recorder = new MediaRecorder(stream, { mimeType: 'video/webm;codecs=vp8' });
    const chunks: Blob[] = [];
    recorder.ondataavailable = event => { if (event.data.size) chunks.push(event.data); };
    const finished = new Promise<Blob>((resolve, reject) => {
      recorder.onstop = () => resolve(new Blob(chunks, { type: 'video/webm' }));
      recorder.onerror = () => reject(new Error('Actual browser video encoder failed'));
    });
    recorder.start(100);
    for (let frame = 0; frame < 18; frame++) {
      context.fillStyle = frame % 2 ? '#3164c8' : '#e69138'; context.fillRect(0, 0, 160, 96);
      context.fillStyle = 'white'; context.fillRect(frame * 7, 12, 12, 60);
      await new Promise(resolve => setTimeout(resolve, 70));
    }
    recorder.stop(); const result = await finished; stream.getTracks().forEach(track => track.stop());
    const bytes = new Uint8Array(await result.arrayBuffer());
    let raw = ''; for (const byte of bytes) raw += String.fromCharCode(byte);
    return btoa(raw);
  });
  const buffer = Buffer.from(encoded, 'base64');
  expect(buffer.subarray(0, 4).toString('hex')).toBe('1a45dfa3');
  return { name: 'PRIVATE_video_capture.webm', mimeType: 'video/webm', buffer, category: 'video', expectedSha256: sha256(buffer) };
}

async function decryptedDownload(actor: EncryptionActor, published: Published) {
  const view = attachment(actor, published.receipt.upload_id);
  const link = view.getByRole('link', { name: `Download ${published.fixture.name}`, exact: true });
  await expect(link).toBeVisible(); await expect(link).toHaveAttribute('href', /^blob:/);
  const waiting = actor.page.waitForEvent('download');
  await link.click(); const downloaded = await waiting;
  expect(downloaded.suggestedFilename()).toBe(published.fixture.name);
  const path = await downloaded.path(); expect(path).not.toBeNull();
  const bytes = readFileSync(path!);
  expect(sha256(bytes), 'Second-browser download must match independently known input/cleaned bytes').toBe(published.fixture.expectedSha256);
  if (published.fixture.category === 'image') expect(bytes.includes(Buffer.from(marker))).toBe(false);
  // These are controlled fixtures, but retain hashes only in the evidence bundle.
  await downloaded.delete();
  return { sha256: sha256(bytes), bytes: bytes.length };
}

test('encrypted image, video and file lifecycle is proven by independent peer bytes and actual storage', async ({ browser }) => {
  const backendURL = process.env.ANNEX_ATTACHMENT_BACKEND_URL!;
  const evidence = attachmentEvidence(backendURL);
  const proxy = await attachmentProxy(backendURL, baseURL);
  const actors: EncryptionActor[] = [];
  const receipts: Record<string, unknown>[] = [];
  const published: Published[] = [];
  let complete = false;
  try {
    const founder = await encryptionActor(browser); actors.push(founder);
    await expect(founder.page.locator('.admin-menu-btn'), 'Use a fresh dedicated captured session database').toBeVisible();
    const member = await encryptionActor(browser); actors.push(member);
    const outsider = await encryptionActor(browser); actors.push(outsider);
    let reader = member;
    const name = `private-files-${Date.now()}`;
    const channel = await createText(founder, name);
    const channelPath = `/api/channels/${channel.channel_id}`;
    const uploadPath = `${channelPath}/encrypted-upload`;
    const reconcile = async (actor: EncryptionActor) => {
      await select(actor, name);
      const check = actor.page.getByRole('button', { name: 'Check again', exact: true });
      if (await check.isVisible()) await check.click();
      await ready(actor);
    };
    const grant = async (actor: EncryptionActor, deviceId?: string) => {
      const response = await actor.context.request.post(new URL('/api/uploads/grant', baseURL).href,
        { headers: actor.headers, data: deviceId ? { device_id: deviceId } : {} });
      expect(response.status()).toBe(200);
      return (await response.json() as { token: string }).token;
    };
    const fetchStatus = async (actor: EncryptionActor, item: Published, token?: string) => {
      const url = new URL(item.receipt.url, baseURL); if (token) url.searchParams.set('t', token);
      const response = await actor.context.request.get(url.href);
      return response.status();
    };
    const fileInput = founder.page.locator('.message-input input[type=file]');
    let uploadCount = 0;
    founder.page.on('request', request => { if (new URL(request.url()).pathname === uploadPath) uploadCount++; });

    await test.step('Ordinary UI enables signed encryption and the independent peer explicitly verifies trust', async () => {
      await select(founder, name); await select(member, name);
      await founder.page.getByRole('button', { name: /Enable encryption/ }).click(); await ready(founder);
      await select(member, name); await trust(member, founder, channel.channel_id, true);
      await reconcile(founder); await reconcile(member);
      const proof = await signedEvidence(member, channel.channel_id);
      receipts.push({ action: 'actual signed trust and two independently certified devices', ...proof.publicReceipt });
    });

    const manifest = JSON.parse(readFileSync(new URL('decoded-cleaned.json', imageDirectory), 'utf8')) as {
      complete: boolean; results: Array<{ file: string; sha256: string; frames: unknown[] }>;
    };
    expect(manifest.complete).toBe(true); expect(manifest.results).toHaveLength(5);
    const types: Record<string, string> = { jpg: 'image/jpeg', png: 'image/png', gif: 'image/gif', webp: 'image/webp' };
    const fixtures: Fixture[] = manifest.results.map(result => {
      const buffer = readFileSync(new URL(result.file, imageDirectory));
      expect(buffer.includes(Buffer.from(marker))).toBe(true);
      return { name: `PRIVATE_${result.file}`, mimeType: types[result.file.split('.').pop()!], buffer,
        category: 'image', expectedSha256: result.sha256 };
    });
    fixtures.push(await recordedVideo(founder.page));
    const file = Buffer.from(`Private attachment contents ${marker}\nSecond browser must receive these exact bytes.\n`);
    fixtures.push({ name: 'PRIVATE_original_filename.txt', mimeType: 'text/plain', buffer: file, category: 'file', expectedSha256: sha256(file) });

    await test.step('Image, video and file preview cancellation sends no upload and creates no server row', async () => {
      const before = uploadCount;
      for (const fixture of [fixtures[0], fixtures[5], fixtures[6]]) {
        await fileInput.setInputFiles({ name: fixture.name, mimeType: fixture.mimeType, buffer: fixture.buffer });
        await expect(founder.page.locator('.image-preview-bar')).toContainText(fixture.name);
        await founder.page.getByTitle('Remove attachment', { exact: true }).click();
        await expect(founder.page.locator('.image-preview-bar')).toHaveCount(0);
      }
      expect(uploadCount).toBe(before); expect(evidence.rows(channel.channel_id)).toEqual([]);
      receipts.push({ action: 'three category previews canceled', actualUploadRequests: 0, persistedUploads: 0 });
    });

    const publish = async (fixture: Fixture, caption: string): Promise<Published> => {
      const start = founder.sent.length;
      await fileInput.setInputFiles({ name: fixture.name, mimeType: fixture.mimeType, buffer: fixture.buffer });
      await expect(founder.page.locator('.image-preview-bar')).toContainText(fixture.name);
      await founder.page.getByPlaceholder('Add a caption (optional)...').fill(caption);
      const pending = founder.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === uploadPath);
      await founder.page.getByRole('button', { name: 'Send', exact: true }).click();
      const response = await pending; expect(response.status()).toBe(200);
      const receipt = await response.json() as UploadReceipt;
      const wire = inspectUpload(proxy.body(receipt.upload_id));
      expect(receipt.encrypted).toBe(true); expect(receipt.filename).toBe('encrypted.bin');
      expect(receipt.content_type).toBe('application/octet-stream'); expect(receipt.category).toBe(fixture.category);
      expect(receipt.upload_id).toBe(wire.fields.attachment_id); expect(receipt.epoch).toBe(Number(wire.fields.epoch));
      expect(receipt.key_id).toBe(wire.fields.key_id); expect(receipt.size).toBe(wire.file.length);
      expect(receipt.ciphertext_sha256).toBe(sha256(wire.file));
      for (const forbidden of [marker, fixture.name, fixture.mimeType, 'fileKeyHex', 'plaintextSha256']) {
        expect(wire.raw.includes(Buffer.from(forbidden)), 'Private descriptor fields must not appear in the multipart request').toBe(false);
      }
      await expect(bubble(founder, caption)).not.toHaveClass(/pending|failed/);
      await expect(bubble(founder, caption).locator('.reply-btn')).toBeVisible();
      await expect.poll(() => founder.sent.slice(start).filter(frame => frame.channelId === channel.channel_id).length).toBe(1);
      const frame = founder.sent.slice(start).find(item => item.channelId === channel.channel_id)! as unknown as { content: string; attachmentIds: string[] };
      expect(frame.content).toMatch(/^e2e2:[1-9][0-9]*:[0-9a-f]{64}:[A-Za-z0-9+/=]+$/);
      expect(frame.attachmentIds).toEqual([receipt.upload_id]);
      for (const forbidden of [caption, marker, fixture.name, 'fileKeyHex']) expect(frame.content.includes(forbidden)).toBe(false);
      let message: Message | undefined;
      await expect.poll(async () => {
        message = (await read<Message[]>(founder, `${channelPath}/messages?limit=100`)).find(value => value.content === frame.content);
        return Boolean(message);
      }).toBe(true);
      expect(evidence.associations(message!.message_id)).toEqual([receipt.upload_id]);
      const stored = evidence.rows(channel.channel_id).find(value => value.upload_id === receipt.upload_id)!;
      expect(stored).toBeDefined(); expect(stored.lifecycle).toBe('ready'); expect(stored.encrypted).toBe(1);
      expect(stored.original_filename).toBe('encrypted.bin'); expect(stored.content_type).toBe('application/octet-stream');
      expect(stored.epoch).toBe(receipt.epoch); expect(stored.key_id).toBe(receipt.key_id);
      expect(stored.ciphertext_sha256).toBe(receipt.ciphertext_sha256); expect(stored.size_bytes).toBe(receipt.size);
      expect(sha256(evidence.bytes(stored))).toBe(sha256(wire.file));
      expect(JSON.stringify(stored).includes(fixture.name)).toBe(false);
      const storedProof = evidence.assertStoredMessage(message!.message_id, frame.content, [caption, fixture.name, marker, 'fileKeyHex']);
      const item = { receipt, stored, messageId: message!.message_id, fixture, caption };
      await expect(bubble(reader, caption)).toBeVisible();
      const download = await decryptedDownload(reader, item);
      expect(receipt.size).toBe(download.bytes + 28);
      receipts.push({ action: 'actual encrypted upload and independent peer download', category: fixture.category,
        uploadId: receipt.upload_id, messageId: item.messageId, inputSha256: sha256(fixture.buffer),
        ciphertextSha256: receipt.ciphertext_sha256, downloadedSha256: download.sha256, ...storedProof });
      return item;
    };

    await test.step('Five actual image formats, recorded video and file cross real encrypted upload, WS, database, disk and peer download', async () => {
      for (let index = 0; index < fixtures.length; index++) {
        const item = await publish(fixtures[index], `private-caption-${index}-${Date.now()}`); published.push(item);
        const view = attachment(member, item.receipt.upload_id);
        if (item.fixture.category === 'image') {
          const image = view.getByAltText(item.fixture.name, { exact: true });
          await expect.poll(() => image.evaluate(element => (element as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
          await view.getByRole('button', { name: `View ${item.fixture.name} full size`, exact: true }).click();
          const dialog = member.page.getByRole('dialog', { name: item.fixture.name, exact: true });
          await expect(dialog.getByAltText(item.fixture.name)).toBeVisible();
          await dialog.getByRole('button', { name: 'Close image', exact: true }).click(); await expect(dialog).toHaveCount(0);
        } else if (item.fixture.category === 'video') {
          const video = view.getByLabel(item.fixture.name, { exact: true });
          await expect.poll(() => video.evaluate(element => (element as HTMLVideoElement).videoWidth)).toBe(160);
          await video.evaluate(async element => { const media = element as HTMLVideoElement; media.muted = true; await media.play(); });
          await expect.poll(() => video.evaluate(element => (element as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames)).toBeGreaterThan(5);
          await video.evaluate(element => (element as HTMLVideoElement).pause());
          receipts.push({ action: 'peer decoded genuine received WebM frames', width: 160, minimumDecodedFrames: 6 });
        }
      }
    });

    await test.step('Editing the caption retains the same ciphertext file and association; peer reload decrypts every attachment', async () => {
      const item = published.at(-1)!; const changed = `${item.caption}-edited`;
      await bubble(founder, item.caption).locator('.edit-btn').click();
      await expect(founder.page.getByLabel(`Remove ${item.fixture.name}`, { exact: true })).not.toBeChecked();
      await founder.page.getByLabel('Edit message', { exact: true }).fill(changed);
      await founder.page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(bubble(member, changed)).toBeVisible(); item.caption = changed;
      expect(evidence.associations(item.messageId)).toEqual([item.receipt.upload_id]);
      expect(sha256(evidence.bytes(item.stored))).toBe(item.receipt.ciphertext_sha256);
      await reload(member); await reconcile(member);
      for (const uploaded of published) await decryptedDownload(member, uploaded);
      receipts.push({ action: 'caption edit retains exact file; fresh page decrypts all seven attachments', attachmentCount: published.length });
    });

    await test.step('A real browser transport refusal exposes Retry attachment, then current-device retry fetches exact bytes', async () => {
      const cdp = await member.context.newCDPSession(member.page);
      await cdp.send('Network.enable');
      try {
        // Browser network failure, with the server response neither replaced
        // nor fulfilled by this test. All other actual API traffic still runs.
        await cdp.send('Network.setBlockedURLs', { urls: [`${baseURL}/uploads/chat/*`] });
        await reload(member); await reconcile(member);
        const failed = attachment(member, published[0].receipt.upload_id);
        await expect(failed.getByRole('alert')).toBeVisible();
        await expect(failed.getByRole('link')).toHaveCount(0);
        await cdp.send('Network.setBlockedURLs', { urls: [] });
        for (const item of published) {
          const view = attachment(member, item.receipt.upload_id);
          await expect(view.getByRole('button', { name: 'Retry attachment', exact: true })).toBeVisible();
          const retried = member.page.waitForResponse(response => response.request().method() === 'POST'
            && new URL(response.url()).pathname === `${channelPath}/uploads/${item.receipt.upload_id}/retry`);
          await view.getByRole('button', { name: 'Retry attachment', exact: true }).click();
          const response = await retried; expect(response.status()).toBe(200);
          expect(response.request().postDataJSON()).toEqual({ device_id: member.deviceId });
          await decryptedDownload(member, item);
        }
        receipts.push({ action: 'actual browser attachment transport refusal and current-device Retry restores seven exact downloads' });
      } finally { await cdp.send('Network.setBlockedURLs', { urls: [] }); await cdp.detach(); }
    });

    await test.step('Real partial-body transport cut fails visibly, retries successfully, and explicit cancellation leaves no row', async () => {
      const bytes = Buffer.alloc(2 * 1024 * 1024, 65); Buffer.from(marker).copy(bytes);
      const fixture: Fixture = { name: 'PRIVATE_interrupted_file.txt', mimeType: 'text/plain', buffer: bytes, category: 'file', expectedSha256: sha256(bytes) };
        await fileInput.setInputFiles({ name: fixture.name, mimeType: fixture.mimeType, buffer: fixture.buffer });
        await founder.page.getByPlaceholder('Add a caption (optional)...').fill('interrupted-upload-retry');
        const interrupted = proxy.holdNextUpload(uploadPath, 'body');
        const started = founder.page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === uploadPath);
        await founder.page.getByRole('button', { name: 'Send', exact: true }).click(); const request = await started;
        const interruptedId = await interrupted.waitUntilHeld();
        const failed = founder.page.waitForEvent('requestfailed', { predicate: item => item === request });
        interrupted.cut();
        await failed;
        await expect(founder.page.getByRole('alert').filter({ hasText: /upload|fetch|network/i })).toBeVisible();
        await expect(founder.page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
        await expect.poll(() => evidence.absentId(interruptedId)).toBe(true);
        await expect(bubble(member, 'interrupted-upload-retry')).toHaveCount(0);
        interrupted.release();
        // The actual failed preview remains selected. Clicking Send again uses
        // the shipped retry path; no test-supplied success response intervenes.
        const retryResponse = founder.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === uploadPath);
        await founder.page.getByRole('button', { name: 'Send', exact: true }).click();
        const retried = await retryResponse; expect(retried.status()).toBe(200);
        const receipt = await retried.json() as UploadReceipt; expect(receipt.upload_id).not.toBe(interruptedId);
        const wire = inspectUpload(proxy.body(receipt.upload_id)); expect(sha256(wire.file)).toBe(receipt.ciphertext_sha256);
        await expect(bubble(member, 'interrupted-upload-retry')).toBeVisible();
        const frame = [...founder.sent].reverse().find(value => value.channelId === channel.channel_id)!;
        const message = (await read<Message[]>(founder, `${channelPath}/messages?limit=100`)).find(value => value.content === frame.content)!;
        expect(message).toBeDefined(); expect(evidence.associations(message.message_id)).toEqual([receipt.upload_id]);
        const stored = evidence.rows(channel.channel_id).find(value => value.upload_id === receipt.upload_id)!;
        const item: Published = { receipt, stored, messageId: message.message_id, fixture, caption: 'interrupted-upload-retry' };
        published.push(item); await decryptedDownload(member, item);
        await fileInput.setInputFiles({ name: 'PRIVATE_cancel_in_flight.txt', mimeType: 'text/plain', buffer: bytes });
        const canceledTransfer = proxy.holdNextUpload(uploadPath, 'body');
        const canceledRequest = founder.page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === uploadPath);
        await founder.page.getByRole('button', { name: 'Send', exact: true }).click(); const canceled = await canceledRequest;
        const canceledId = await canceledTransfer.waitUntilHeld();
        const aborted = founder.page.waitForEvent('requestfailed', { predicate: item => item === canceled });
        await founder.page.getByTitle('Remove attachment', { exact: true }).click(); await aborted;
        await expect(founder.page.locator('.image-preview-bar')).toHaveCount(0);
        await expect.poll(() => evidence.absentId(canceledId)).toBe(true);
        receipts.push({ action: 'real transport interruption then actual Send retry and in-flight cancellation', interruptedId, canceledId, successfulRetryId: receipt.upload_id });

        // Exercise the separate lost-ack boundary: the actual server commits
        // the ciphertext while its actual response is held by the forwarding
        // transport. Explicit cancellation must clean this UUID, not wait for
        // the orphan TTL or infer that aborting fetch canceled a remote commit.
        await fileInput.setInputFiles({ name: 'PRIVATE_cancel_after_commit.txt', mimeType: 'text/plain', buffer: bytes });
        await founder.page.getByPlaceholder('Add a caption (optional)...').fill('cancel-after-actual-commit');
        const sentBefore = founder.sent.length;
        const acknowledgement = proxy.holdNextUpload(uploadPath, 'response');
        const lostAck = founder.page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === uploadPath);
        await founder.page.getByRole('button', { name: 'Send', exact: true }).click();
        const committedRequest = await lostAck;
        const committedId = await acknowledgement.waitUntilHeld();
        let committedRow: UploadRow | undefined;
        await expect.poll(() => {
          committedRow = evidence.rows(channel.channel_id).find(value => value.upload_id === committedId && value.lifecycle === 'ready');
          return Boolean(committedRow);
        }, { timeout: 45_000, message: 'The actual server must commit this upload while its actual HTTP acknowledgement is held' }).toBe(true);
        await expect(founder.page.getByPlaceholder('Uploading...'), 'The lost-ack setup must still be awaiting the real response').toBeVisible();
        const lostAckAborted = founder.page.waitForEvent('requestfailed', { predicate: item => item === committedRequest });
        await founder.page.getByTitle('Remove attachment', { exact: true }).click(); await lostAckAborted;
        await expect.poll(() => evidence.gone(committedRow!), { timeout: 65_000,
          message: 'Explicit cancellation after a real remote commit must delete the staged upload and its actual disk bytes' }).toBe(true);
        expect(founder.sent).toHaveLength(sentBefore);
        await expect(bubble(member, 'cancel-after-actual-commit')).toHaveCount(0);
        receipts.push({ action: 'explicit cancellation after actual SQLite/file commit and before withheld actual HTTP acknowledgement removes row and bytes', uploadId: committedId });
    });

    await test.step('Fresh grants require the correct certified device and current membership', async () => {
      const item = published[0];
      expect(await fetchStatus(member, item)).toBe(401);
      expect(await fetchStatus(member, item, await grant(member))).toBe(403);
      expect(await fetchStatus(outsider, item, await grant(outsider))).toBe(404);
      const wrongDevice = await outsider.context.request.post(new URL('/api/uploads/grant', baseURL).href,
        { headers: outsider.headers, data: { device_id: member.deviceId } }); expect(wrongDevice.status()).toBe(403);
      const token = await grant(member, member.deviceId!); expect(await fetchStatus(member, item, token)).toBe(200);
      const leaving = member.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === `${channelPath}/leave`);
      await row(member, name).locator('.leave-btn').click(); expect((await leaving).status()).toBe(200);
      expect(await fetchStatus(member, item, token)).toBe(404);
      const retry = await member.context.request.post(new URL(`${channelPath}/uploads/${item.receipt.upload_id}/retry`, baseURL).href,
        { headers: member.headers, data: { device_id: member.deviceId } }); expect(retry.status()).toBe(403);
      await select(member, name); await reconcile(founder); await reconcile(member); await decryptedDownload(member, item);
      receipts.push({ action: 'missing grant, v1 encrypted grant, nonmember, mismatched device and former member denied; genuine rejoin restores download' });
    });

    await test.step('Approved linked device reads old attachment, then removes the original device and its prior grant is refused', async () => {
      const originalDevice = member.deviceId!;
      const originalGrant = await grant(member, originalDevice);
      const linked = await encryptionActor(browser, false); actors.push(linked);
      await linkDevice(member, linked);
      expect((await publicIdentities(linked))[0].pseudonymId).toBe((await publicIdentities(member))[0].pseudonymId);
      await select(linked, name); await trust(linked, founder, channel.channel_id);
      await expect.poll(() => linked.deviceId).toMatch(/^[0-9a-f]{64}$/); expect(linked.deviceId).not.toBe(originalDevice);
      await member.page.getByRole('button', { name: 'Reconnect here', exact: true }).click(); await reconcile(member);
      await member.page.getByRole('button', { name: 'Encryption devices', exact: true }).click();
      let dialog = member.page.getByRole('dialog', { name: 'Encryption devices', exact: true });
      const candidate = dialog.locator('.encryption-device').filter({ has: member.page.locator('code', { hasText: linked.deviceId! }) });
      await candidate.getByRole('button', { name: 'Approve device', exact: true }).click(); await expect(candidate).toContainText('Approved');
      await dialog.getByRole('button', { name: 'Done', exact: true }).click();
      await linked.page.getByRole('button', { name: 'Reconnect here', exact: true }).click(); await reconcile(linked);
      await decryptedDownload(linked, published[0]); reader = linked;
      // Read grants do not depend on being the foreground socket; establish
      // that the old device grant still works before the signed removal.
      expect(await fetchStatus(member, published[0], originalGrant)).toBe(200);
      await linked.page.getByRole('button', { name: 'Encryption devices', exact: true }).click();
      dialog = linked.page.getByRole('dialog', { name: 'Encryption devices', exact: true });
      const old = dialog.locator('.encryption-device').filter({ has: linked.page.locator('code', { hasText: originalDevice }) });
      await old.getByRole('button', { name: 'Remove from channel', exact: true }).click();
      await old.getByRole('button', { name: 'Cancel removal', exact: true }).click();
      expect(await fetchStatus(member, published[0], originalGrant)).toBe(200);
      await old.getByRole('button', { name: 'Remove from channel', exact: true }).click();
      await old.getByRole('button', { name: 'Confirm removal', exact: true }).click(); await expect(old).toContainText('Removed');
      await dialog.getByRole('button', { name: 'Done', exact: true }).click(); await reconcile(founder); await reconcile(linked);
      expect(await fetchStatus(member, published[0], originalGrant)).toBe(403);
      const freshOldGrant = await grant(member, originalDevice);
      expect(await fetchStatus(member, published[0], freshOldGrant)).toBe(403);
      await decryptedDownload(linked, published[0]);
      receipts.push({ action: 'real linked device approval, canceled removal, confirmed signed removal and old/fresh grants denied',
        originalDevice, linkedDevice: linked.deviceId, ...(await signedEvidence(linked, channel.channel_id)).publicReceipt });
    });

    await test.step('Attachment removal during edit and confirmed message deletion revoke URLs and drain real disk cleanup', async () => {
      await reconcile(founder);
      const token = await grant(reader, reader.deviceId!);
      // The actual product permits author edits/deletes for 60 seconds. Use
      // fresh messages, then delete the channel for older retained files.
      const editRemoved = await publish(fixtures[6], `fresh-edit-removal-${Date.now()}`); published.push(editRemoved);
      await bubble(founder, editRemoved.caption).locator('.edit-btn').click();
      await founder.page.getByLabel(`Remove ${editRemoved.fixture.name}`, { exact: true }).check();
      await founder.page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(attachment(reader, editRemoved.receipt.upload_id)).toHaveCount(0);
      expect(evidence.associations(editRemoved.messageId)).toEqual([]);
      expect(await fetchStatus(reader, editRemoved, token)).toBe(404);
      await expect.poll(() => evidence.gone(editRemoved.stored), { timeout: 65_000 }).toBe(true);
      const deleted = await publish(fixtures[6], `fresh-message-deletion-${Date.now()}`); published.push(deleted);
      const message = bubble(founder, deleted.caption);
      await message.locator('.delete-btn').click(); await message.locator('.delete-btn.confirming').click();
      await expect(attachment(reader, deleted.receipt.upload_id)).toHaveCount(0);
      expect(evidence.associations(deleted.messageId)).toEqual([]);
      expect(await fetchStatus(reader, deleted, token)).toBe(404);
      await founder.page.locator('.admin-menu-btn').click();
      await founder.page.getByRole('button', { name: 'Channel Management', exact: true }).click();
      const entry = founder.page.locator('.channel-manager-item', { hasText: name });
      await entry.getByRole('button', { name: 'Delete', exact: true }).click();
      await founder.page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
      expect(await fetchStatus(reader, published[0], token)).toBe(200);
      await entry.getByRole('button', { name: 'Delete', exact: true }).click();
      const channelDeleted = founder.page.waitForResponse(response => response.request().method() === 'DELETE'
        && new URL(response.url()).pathname === channelPath);
      await founder.page.getByRole('dialog').getByRole('button', { name: 'Delete channel', exact: true }).click();
      expect((await channelDeleted).status()).toBe(200);
      await expect(row(reader, name)).toHaveCount(0);
      for (const item of published) expect(await fetchStatus(reader, item, token)).toBe(404);
      await expect.poll(() => published.every(item => evidence.gone(item.stored)), { timeout: 65_000 }).toBe(true);
      expect(evidence.rows(channel.channel_id)).toEqual([]);
      receipts.push({ action: 'fresh edit removal, confirmed message deletion and canceled/confirmed channel deletion invalidate grants, associations, blobs and cleanup obligations', attachmentCount: published.length });
    });
    for (const actor of actors) expect(actor.errors, 'Uncaught browser errors must remain empty before a complete receipt').toEqual([]);
    complete = true;
  } finally {
    try {
      await test.info().attach('encrypted-attachment-evidence', { body: JSON.stringify({ complete, binarySha256: evidence.binarySha256,
        boundary: 'Actual browser UI, proofs and unchanged forwarded request bytes. Controlled fixtures; no physical camera/microphone claim. Read-only SQLite; no seeded records or mocked responses.',
        forwarding: proxy.publicReceipt(), receipts }, null, 2), contentType: 'application/json' });
    } finally {
      for (const actor of actors) { expect.soft(actor.errors, 'Uncaught browser errors').toEqual([]); await actor.context.close(); }
      evidence.close(); await proxy.close();
    }
  }
});
