import 'fake-indexeddb/auto';
import { IDBFactory } from 'fake-indexeddb';
import { openDB } from 'idb';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { clearAllDatabases, listIdentities, saveIdentity } from './db';
import { createServerEntry, listServers, saveServer } from './servers';
import { createPersona, listPersonas } from './personas';

vi.mock('idb', async (importOriginal) => {
  const actual = await importOriginal<typeof import('idb')>();
  return { ...actual, openDB: vi.fn(actual.openDB) };
});

describe('storage recovery', () => {
  beforeEach(() => {
    vi.stubGlobal('indexedDB', new IDBFactory());
    vi.mocked(openDB).mockClear();
  });

  afterEach(async () => {
    await clearAllDatabases();
    vi.unstubAllGlobals();
  });

  it.each([
    ['identity', listIdentities],
    ['servers', listServers],
    ['personas', listPersonas],
  ] as const)('reopens %s after a rejected native open', async (_name, list) => {
    vi.mocked(openDB).mockRejectedValueOnce(new DOMException('Native storage lock', 'UnknownError'));
    await expect(list()).rejects.toThrow('Native storage lock');
    await expect(list()).resolves.toEqual([]);
    expect(openDB).toHaveBeenCalledTimes(2);
  });

  it('closes every owned handle, removes records, and preserves unrelated databases', async () => {
    await saveIdentity({
      id: 'identity', sk: 'test-secret', roleCode: 1, nodeId: 1,
      commitmentHex: 'commitment', pseudonymId: null, sessionToken: null,
      serverSlug: '', leafIndex: null, createdAt: new Date().toISOString(),
    });
    await saveServer(createServerEntry('https://example.test', 'test', 'Test', 'identity'));
    await createPersona('Test', 'identity', 'test');
    const unrelated = await openDB('unrelated', 1, { upgrade(db) { db.createObjectStore('data'); } });
    await unrelated.put('data', 'retain', 'key');

    await expect(clearAllDatabases()).resolves.toEqual({ unremoved: [] });
    await expect(listIdentities()).resolves.toEqual([]);
    await expect(listServers()).resolves.toEqual([]);
    await expect(listPersonas()).resolves.toEqual([]);
    await expect(unrelated.get('data', 'key')).resolves.toBe('retain');
    unrelated.close();
  });

  it('reports a real external connection blocking deletion, then recovers when it closes', async () => {
    await listIdentities();
    const external = await openDB('annex-identity', 1);
    try {
      await expect(clearAllDatabases()).resolves.toEqual({ unremoved: ['annex-identity'] });
    } finally {
      external.close();
    }
    await expect(clearAllDatabases()).resolves.toEqual({ unremoved: [] });
    await expect(listIdentities()).resolves.toEqual([]);
  });

  it('releases its own connection for external deletion and reopens on the next operation', async () => {
    await listIdentities();
    await new Promise<void>((resolve, reject) => {
      const request = indexedDB.deleteDatabase('annex-identity');
      request.onsuccess = () => resolve();
      request.onerror = () => reject(request.error);
      request.onblocked = () => reject(new Error('Owned identity handle blocked deletion'));
    });
    await expect(listIdentities()).resolves.toEqual([]);
  });
});
