import { createConnection, createServer, type Socket } from 'node:net';

/** A byte-for-byte TCP relay to the real HTTP/WebSocket server. It never
 * interprets requests, manufactures responses, or changes application data.
 * hold() models a stalled link; cut() drops the actual TCP connections. */
export async function realNetworkLink(targetUrl: string) {
  const target = new URL(targetUrl);
  if (target.protocol !== 'http:' || target.pathname !== '/' || target.search || target.hash) {
    throw new Error('Resilience lane requires an actual HTTP server origin');
  }
  type Pair = { client: Socket; upstream?: Socket };
  const pairs = new Set<Pair>();
  let mode: 'online' | 'held' | 'offline' = 'online';
  const connect = (pair: Pair) => {
    const upstream = createConnection({ host: target.hostname, port: Number(target.port || 80) });
    pair.upstream = upstream;
    upstream.on('error', () => pair.client.destroy());
    upstream.on('close', () => pair.client.destroy());
    pair.client.pipe(upstream);
    upstream.pipe(pair.client);
    if (mode === 'held') { pair.client.pause(); upstream.pause(); }
  };
  const server = createServer(client => {
    if (mode === 'offline') { client.destroy(); return; }
    const pair: Pair = { client };
    pairs.add(pair);
    client.on('error', () => pair.upstream?.destroy());
    client.on('close', () => { pair.upstream?.destroy(); pairs.delete(pair); });
    if (mode === 'held') client.pause();
    else connect(pair);
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => { server.off('error', reject); resolve(); });
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('TCP relay has no assigned port');
  const cut = () => {
    mode = 'offline';
    for (const pair of pairs) { pair.client.destroy(); pair.upstream?.destroy(); }
  };
  return {
    url: `http://127.0.0.1:${address.port}`,
    hold() {
      mode = 'held';
      for (const pair of pairs) { pair.client.pause(); pair.upstream?.pause(); }
    },
    cut,
    resume() {
      mode = 'online';
      for (const pair of pairs) {
        if (!pair.upstream) connect(pair);
        pair.client.resume(); pair.upstream?.resume();
      }
    },
    async close() {
      cut();
      await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    },
  };
}
export type RealNetworkLink = Awaited<ReturnType<typeof realNetworkLink>>;
