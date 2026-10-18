import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
const { fetchBeaconJson } = createRequire(import.meta.url)('../../zk/scripts/beacon-fetch.js');

async function fixture(t, handler) {
  const server = createServer(handler);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  return `http://127.0.0.1:${server.address().port}/beacon`;
}

test('temporary service failures recover without changing the returned beacon', async t => {
  let requests = 0;
  const url = await fixture(t, (_request, response) => {
    if (++requests < 3) { response.writeHead(503); response.end('unavailable'); }
    else { response.end(JSON.stringify({ round: 42, signature: 'unchanged fixture' })); }
  });
  assert.deepEqual(await fetchBeaconJson(url, { retryDelayMs: 0 }), { round: 42, signature: 'unchanged fixture' });
  assert.equal(requests, 3);
});

test('a stalled response is bounded and exhaustion remains a failure', async t => {
  let requests = 0;
  const url = await fixture(t, (_request, response) => {
    requests++; response.writeHead(200); response.write('{');
  });
  await assert.rejects(fetchBeaconJson(url, { timeoutMs: 100, retryDelayMs: 0 }));
  assert.equal(requests, 3);
});

test('malformed beacon data and permanent HTTP refusal are not retried into success', async t => {
  for (const status of [200, 404]) {
    let requests = 0;
    const url = await fixture(t, (_request, response) => {
      requests++; response.writeHead(status); response.end('invalid JSON');
    });
    await assert.rejects(fetchBeaconJson(url, { retryDelayMs: 0 }));
    assert.equal(requests, 1);
  }
});
