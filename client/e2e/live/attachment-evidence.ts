import assert from 'node:assert/strict';
import { createHash, createDecipheriv, hkdfSync } from 'node:crypto';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, isAbsolute, join, relative } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

export const sha256 = (bytes: string | Buffer) => createHash('sha256').update(bytes).digest('hex');
export interface UploadReceipt {
  upload_id: string; url: string; filename: string; content_type: string; category: string;
  size: number; encrypted: boolean; epoch: number; key_id: string; ciphertext_sha256: string;
}
export interface UploadRow {
  upload_id: string; original_filename: string; content_type: string; size_bytes: number;
  category: string; encrypted: number; epoch: number; key_id: string; ciphertext_sha256: string;
  lifecycle: string; storage_path: string; staging_path: string | null;
}

/** Independent multipart decoding of the actual forwarded browser request. Never imports
 * the product encryption/serialization implementation and never reads its keys. */
export function inspectUpload({ contentType, raw }: { contentType: string; raw: Buffer }) {
  const boundary = /boundary=(?:"([^"]+)"|([^;]+))/.exec(contentType ?? '');
  assert(boundary, 'The actual upload must be multipart');
  const separator = Buffer.from(`--${boundary[1] ?? boundary[2]}`);
  const fields: Record<string, string> = {};
  let file: Buffer | undefined;
  let position = 0;
  while ((position = raw.indexOf(separator, position)) >= 0) {
    position += separator.length;
    if (raw.subarray(position, position + 2).equals(Buffer.from('--'))) break;
    assert(raw.subarray(position, position + 2).equals(Buffer.from('\r\n')));
    const headerEnd = raw.indexOf(Buffer.from('\r\n\r\n'), position + 2);
    assert(headerEnd >= 0);
    const headers = raw.subarray(position + 2, headerEnd).toString('utf8');
    const end = raw.indexOf(Buffer.concat([Buffer.from('\r\n'), separator]), headerEnd + 4);
    assert(end >= 0);
    const bytes = raw.subarray(headerEnd + 4, end);
    const name = /(?:^|;)\s*name="([^"]+)"/.exec(headers.split('\r\n')[0])?.[1];
    assert(name);
    if (name === 'file') {
      assert(!file, 'Exactly one ciphertext file');
      assert(headers.includes(`filename="${fields.attachment_id}.bin"`));
      assert(/Content-Type: application\/octet-stream/i.test(headers));
      file = Buffer.from(bytes);
    } else {
      assert(!file && !(name in fields), 'Only unique metadata before the file');
      fields[name] = bytes.toString('utf8');
    }
    position = end + 2;
  }
  assert.deepEqual(Object.keys(fields), ['attachment_id', 'epoch', 'key_id', 'category']);
  assert(file && file.length >= 28);
  return { fields, file, raw };
}

/** Read-only inspection is restricted to the root-created disposable session.
 * Server at-rest decryption happens only in memory and yields the exact E2E
 * ciphertext, never a client attachment key or private descriptor. */
export function attachmentEvidence(baseURL: string) {
  const recordPath = process.env.ANNEX_ATTACHMENT_SESSION;
  assert(recordPath, 'ANNEX_ATTACHMENT_SESSION must identify a fresh captured native session');
  const session = JSON.parse(readFileSync(recordPath, 'utf8').replace(/^\uFEFF/, '')) as {
    data: string; url: string; binary: string; binary_sha256: string;
  };
  assert.equal(new URL(session.url).origin, new URL(baseURL).origin);
  const sessions = realpathSync(join(dirname(recordPath), 'sessions'));
  const data = realpathSync(session.data);
  const confined = relative(sessions, data);
  assert(confined && !confined.startsWith('..') && !isAbsolute(confined), 'Only isolated acceptance storage may be inspected');
  assert.equal(sha256(readFileSync(session.binary)), session.binary_sha256.toLowerCase());
  const root = join(data, 'uploads');
  const diskPath = (path: string) => {
    assert(path && !path.split(/[\\/]/).includes('..') && !isAbsolute(path));
    const full = join(root, path);
    assert(!relative(root, full).startsWith('..'));
    return full;
  };
  const db = new DatabaseSync(join(data, 'annex.db'), { readOnly: true });
  return {
    binarySha256: session.binary_sha256.toLowerCase(),
    rows(channel: string) {
      return db.prepare('SELECT upload_id,original_filename,content_type,size_bytes,category,encrypted,epoch,key_id,ciphertext_sha256,lifecycle,storage_path,staging_path FROM uploads WHERE channel_id=? ORDER BY upload_id').all(channel) as unknown as UploadRow[];
    },
    associations(messageId: string) {
      return db.prepare('SELECT upload_id FROM message_uploads WHERE message_id=? ORDER BY upload_id').all(messageId).map(row => row.upload_id);
    },
    absentId(id: string) {
      assert(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id));
      const row = db.prepare('SELECT count(*) AS n FROM uploads WHERE upload_id=?').get(id)!;
      return Number(row.n) === 0 && ['images', 'videos', 'files'].every(category => !existsSync(diskPath(`chat/${category}/${id}.bin`)))
        && !existsSync(diskPath(`.staging/${id}.part`));
    },
    bytes(row: UploadRow) { return readFileSync(diskPath(row.storage_path)); },
    gone(row: UploadRow) {
      const count = db.prepare('SELECT count(*) AS n FROM uploads WHERE upload_id=?').get(row.upload_id)!;
      const queued = db.prepare('SELECT count(*) AS n FROM upload_deletion_queue WHERE relative_path=? OR relative_path=?').get(row.storage_path, row.staging_path ?? '')!;
      return Number(count.n) === 0 && Number(queued.n) === 0 && !existsSync(diskPath(row.storage_path))
        && (!row.staging_path || !existsSync(diskPath(row.staging_path)));
    },
    assertStoredMessage(messageId: string, wire: string, forbidden: string[]) {
      const row = db.prepare('SELECT content FROM messages WHERE message_id=?').get(messageId);
      assert(row && typeof row.content === 'string');
      const raw = row.content;
      for (const value of forbidden) assert(!raw.includes(value), 'Private attachment metadata must not appear in stored message bytes');
      assert(raw.startsWith('\x01ar1:'), 'The database must retain authenticated encryption at rest');
      const seed = Buffer.from(readFileSync(join(data, 'signing.key'), 'utf8').trim(), 'hex');
      const key = Buffer.from(hkdfSync('sha256', seed, Buffer.from('annex-message-at-rest'), Buffer.from('annex-message-at-rest-v1'), 32));
      try {
        const bytes = Buffer.from(raw.slice(5), 'base64');
        const decipher = createDecipheriv('chacha20-poly1305', key, bytes.subarray(0, 12), { authTagLength: 16 });
        decipher.setAuthTag(bytes.subarray(-16));
        const actual = Buffer.concat([decipher.update(bytes.subarray(12, -16)), decipher.final()]).toString('utf8');
        assert.equal(sha256(actual), sha256(wire), 'The stored authenticated body must be the exact E2E message observed on the browser socket');
        assert(actual.startsWith('e2e2:'));
        for (const value of forbidden) assert(!actual.includes(value), 'Private attachment metadata must remain inside E2E ciphertext');
        return { storedSha256: sha256(raw), wireSha256: sha256(actual) };
      } finally { seed.fill(0); key.fill(0); }
    },
    close() { db.close(); },
  };
}
