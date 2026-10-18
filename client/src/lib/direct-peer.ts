import { clientHello, type TunnelCipher } from './host-tunnel-crypto';

const PROTOCOL = 'annex-direct-v3';
const peers = new Map<string, Promise<RTCPeerConnection>>();
const MAX_SDP = 24 * 1024;
const DEADLINE = 25_000;

function waitForChannel(channel: RTCDataChannel): Promise<void> {
  if (channel.readyState === 'open') return Promise.resolve();
  return new Promise((resolve, reject) => {
    const cleanup = () => { clearTimeout(timer); channel.removeEventListener('open', open); channel.removeEventListener('close', close); channel.removeEventListener('error', close); };
    const open = () => { cleanup(); resolve(); };
    const close = () => { cleanup(); reject(new Error('The direct peer connection closed.')); };
    const timer = setTimeout(() => { cleanup(); channel.close(); reject(new Error('The direct peer connection timed out.')); }, DEADLINE);
    channel.addEventListener('open', open, { once: true });
    channel.addEventListener('close', close, { once: true }); channel.addEventListener('error', close, { once: true });
  });
}

export async function gatherDirectOffer(pc: RTCPeerConnection): Promise<string> {
  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
  if (pc.iceGatheringState !== 'complete') await new Promise<void>((resolve, reject) => {
    const cleanup = () => { clearTimeout(timer); pc.removeEventListener('icegatheringstatechange', check); };
    const check = () => { if (pc.iceGatheringState === 'complete') { cleanup(); resolve(); } };
    // A disconnected adapter can keep gathering pending after usable direct
    // candidates are already present. Send the candidates collected within the
    // budget; the real connectivity checks still have to establish the peer.
    const timer = setTimeout(() => {
      cleanup();
      if (directCandidates(pc.localDescription?.sdp ?? '')) resolve();
      else reject(new Error('No direct connection address was found. Check the device network and try again.'));
    }, 10_000);
    pc.addEventListener('icegatheringstatechange', check); check();
  });
  const sdp = pc.localDescription?.sdp;
  if (!sdp || sdp.length > MAX_SDP || !directCandidates(sdp)) throw new Error('No usable direct peer address was found.');
  return sdp;
}

function directCandidates(sdp: string): boolean {
  const candidates = sdp.split(/\r?\n/).filter(line => line.startsWith('a=candidate:'));
  return candidates.some(line => /\btyp (?:host|srflx|prflx)\b/.test(line))
    && !candidates.some(line => /\btyp relay\b/.test(line));
}

function answerDescription(value: unknown): RTCSessionDescriptionInit {
  if (!value || typeof value !== 'object') throw new Error('Invalid direct peer answer.');
  const answer = value as Record<string, unknown>;
  if (Object.keys(answer).length !== 3 || answer.protocol !== PROTOCOL || answer.type !== 'answer'
    || typeof answer.sdp !== 'string' || answer.sdp.length > MAX_SDP || !answer.sdp.startsWith('v=0\r\n')
    || answer.sdp.split(/\r?\n/).some(line => line.startsWith('a=candidate:') && /\btyp relay\b/.test(line))) {
    throw new Error('The host did not provide a direct peer connection.');
  }
  return { type: 'answer', sdp: answer.sdp };
}

function introduce(socketUrl: string, host: string): Promise<RTCPeerConnection> {
  return new Promise((resolve, reject) => {
    // STUN discovers peer addresses. There is deliberately no TURN fallback.
    const pc = new RTCPeerConnection({ iceServers: [{ urls: ['stun:stun.l.google.com:19302'] }] });
    const control = pc.createDataChannel('annex-peer-v3');
    const hello = clientHello();
    const socket = new WebSocket(socketUrl);
    let cipher: TunnelCipher | undefined, answered = false, complete = false;
    let queue = Promise.resolve();
    const fail = (error: unknown) => {
      if (complete) return; complete = true; clearTimeout(timer);
      hello.destroy(); cipher?.destroy(); socket.close(); pc.close(); reject(error);
    };
    const timer = setTimeout(() => fail(new Error('Could not establish a direct peer connection. Check the peers’ network reachability.')), DEADLINE);
    socket.onopen = () => socket.send(JSON.stringify({ type: 'tunnel_open', hello: hello.hello }));
    socket.onerror = () => fail(new Error('Peer introduction is unavailable.'));
    socket.onclose = () => { void queue.then(() => { if (!answered) fail(new Error('Peer introduction ended before a direct connection was established.')); }); };
    socket.onmessage = event => {
      queue = queue.then(async () => {
        if (complete) return;
        if (typeof event.data !== 'string' || event.data.length > 64 * 1024) throw new Error('Invalid introduction packet.');
        const packet = JSON.parse(event.data);
        if (!cipher && packet.type === 'tunnel_hello') {
          cipher = hello.finish(host, packet.hello);
          const sdp = await gatherDirectOffer(pc);
          if (complete) return;
          const packets = cipher.seal({ protocol: PROTOCOL, type: 'offer', sdp });
          if (packets.length !== 1) throw new Error('Peer introduction exceeds its limit.');
          socket.send(JSON.stringify({ type: 'peer_offer', ...packets[0] }));
        } else if (cipher && !answered && packet.type === 'peer_answer') {
          const answer = answerDescription(cipher.open(packet));
          answered = true;
          await pc.setRemoteDescription(answer);
          socket.close(1000, 'Peers introduced');
          cipher.destroy(); hello.destroy();
          await waitForChannel(control);
          if (complete) return;
          complete = true; clearTimeout(timer); resolve(pc);
        } else throw new Error('Unexpected introduction packet.');
      }).catch(fail);
    };
  });
}

/** One introduction per live peer, followed by independent ordered channels.
 * Every subsequent request uses the existing peer even if Monolith is offline.
 */
export async function openDirectChannel(socketUrl: string, host: string): Promise<RTCDataChannel> {
  const key = `${host}|${socketUrl}`;
  let pending = peers.get(key);
  if (!pending) {
    if (peers.size >= 64) throw new Error('Too many direct server connections.');
    pending = introduce(socketUrl, host);
    peers.set(key, pending);
    const current = pending;
    void pending.then(pc => {
      let disconnected: ReturnType<typeof setTimeout> | undefined;
      const retire = () => {
        clearTimeout(disconnected);
        if (peers.get(key) === current) peers.delete(key);
        pc.removeEventListener('connectionstatechange', changed); pc.close();
      };
      const changed = () => {
        clearTimeout(disconnected);
        if (pc.connectionState === 'disconnected') disconnected = setTimeout(retire, DEADLINE);
        if (['failed', 'closed'].includes(pc.connectionState)) retire();
      };
      pc.addEventListener('connectionstatechange', changed); changed();
    }, () => { if (peers.get(key) === current) peers.delete(key); });
  }
  const pc = await pending;
  const channel = pc.createDataChannel('annex-application-v3');
  try { await waitForChannel(channel); return channel; }
  catch (error) { channel.close(); throw error; }
}

export function closeDirectPeers(): void {
  for (const pending of peers.values()) void pending.then(pc => pc.close(), () => {});
  peers.clear();
}
