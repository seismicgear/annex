import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { WsReceiveFrame } from '@/types';
import type { AnnexWebSocket } from '@/lib/ws';
import type { WebRtcSession } from '@/lib/webrtc';
import { useWebRtcSignals } from './useWebRtcSignals';

describe('identity-wide media revocation', () => {
  it('disconnects a live or still-connecting native session without a channel ID', () => {
    let receive!: (frame: WsReceiveFrame) => void;
    const unsubscribe = vi.fn();
    const ws = { onMessage: (handler: typeof receive) => { receive = handler; return unsubscribe; } } as unknown as AnnexWebSocket;
    const disconnect = vi.fn();
    const ref = { current: { disconnect } as unknown as WebRtcSession | null };
    const { unmount } = renderHook(() => useWebRtcSignals(ws, ref, 'current-call'));
    act(() => receive({ type: 'identity_capabilities_changed', canVoice: false }));
    expect(disconnect).toHaveBeenCalledOnce(); expect(ref.current).toBeNull();
    act(() => receive({ type: 'identity_capabilities_changed', canVoice: true }));
    expect(ref.current).toBeNull(); expect(disconnect).toHaveBeenCalledOnce();
    unmount(); expect(unsubscribe).toHaveBeenCalledOnce();
  });
  it('never treats a restore hint as a media command or accepts an invalid boolean', () => {
    let receive!: (frame: WsReceiveFrame) => void;
    const ws = { onMessage: (handler: typeof receive) => { receive = handler; return vi.fn(); } } as unknown as AnnexWebSocket;
    const disconnect = vi.fn(); const session = { disconnect } as unknown as WebRtcSession;
    const ref = { current: session as WebRtcSession | null };
    renderHook(() => useWebRtcSignals(ws, ref, 'current-call'));
    act(() => receive({ type: 'identity_capabilities_changed', canVoice: true }));
    act(() => receive({ type: 'identity_capabilities_changed' }));
    expect(disconnect).not.toHaveBeenCalled(); expect(ref.current).toBe(session);
  });
});
