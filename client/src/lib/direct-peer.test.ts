import { afterEach, expect, it, vi } from 'vitest';
import { gatherDirectOffer } from './direct-peer';

class GatheringPeer extends EventTarget {
  iceGatheringState: RTCIceGatheringState = 'gathering';
  localDescription = { type: 'offer', sdp: 'v=0\r\n' };
  createOffer = async () => ({ type: 'offer', sdp: 'v=0\r\n' });
  setLocalDescription = async () => {};
  asPeer() { return this as unknown as RTCPeerConnection; }
}
afterEach(() => vi.useRealTimers());

it('uses collected direct addresses when one adapter never finishes gathering', async () => {
  vi.useFakeTimers();
  const peer = new GatheringPeer();
  const result = gatherDirectOffer(peer.asPeer());
  await vi.advanceTimersByTimeAsync(100);
  peer.localDescription.sdp += 'a=candidate:1 1 UDP 2122260223 192.0.2.10 50123 typ host\r\n';
  await vi.advanceTimersByTimeAsync(9_900);
  expect(await result).toBe(peer.localDescription.sdp);
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['', 'a=candidate:1 1 UDP 1 192.0.2.20 50124 typ relay\r\n'])(
  'does not claim a direct address when none is available (%s)', async candidate => {
    vi.useFakeTimers();
    const peer = new GatheringPeer(); peer.localDescription.sdp += candidate;
    const result = expect(gatherDirectOffer(peer.asPeer())).rejects.toThrow('No direct connection address');
    await vi.advanceTimersByTimeAsync(10_000); await result;
    expect(vi.getTimerCount()).toBe(0);
  },
);

it('finishes immediately when all direct addresses have been collected', async () => {
  vi.useFakeTimers();
  const peer = new GatheringPeer();
  const result = gatherDirectOffer(peer.asPeer());
  await vi.advanceTimersByTimeAsync(20);
  peer.localDescription.sdp += 'a=candidate:2 1 UDP 1 198.51.100.10 50125 typ srflx raddr 192.0.2.10 rport 50123\r\n';
  peer.iceGatheringState = 'complete'; peer.dispatchEvent(new Event('icegatheringstatechange'));
  expect(await result).toBe(peer.localDescription.sdp);
  expect(vi.getTimerCount()).toBe(0);
});
