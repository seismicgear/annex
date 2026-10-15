import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { useVoiceStore } from '@/stores/voice';
import { useRemoteAudio } from './useRemoteAudio';

function audio(remote = true) {
  const el = document.createElement('audio');
  if (remote) el.dataset.webrtcRemote = '';
  el.setSinkId = vi.fn<(id: string) => Promise<void>>(async () => {});
  document.body.append(el);
  return el;
}

describe('call output device routing', () => {
  beforeEach(() => {
    useVoiceStore.setState({ outputDeviceId: null, outputDeviceError: null, outputVolume: 60, deafened: false });
  });
  afterEach(() => { cleanup(); document.body.replaceChildren(); });

  it('only changes call audio, applies newly inserted call nodes, and does not reroute for volume changes', async () => {
    const unrelated = audio(false);
    const call = audio();
    renderHook(() => useRemoteAudio());
    await waitFor(() => expect(call.setSinkId).toHaveBeenCalledWith(''));
    expect(call.volume).toBe(0.6);
    expect(unrelated.volume).toBe(1);
    expect(unrelated.setSinkId).not.toHaveBeenCalled();
    act(() => useVoiceStore.getState().toggleDeafen());
    let later!: HTMLAudioElement;
    await act(async () => { later = audio(); audio(false); });
    expect(later.muted).toBe(true);
    expect(later.volume).toBe(0);
    expect(call.setSinkId).toHaveBeenCalledTimes(1);
    expect(unrelated.muted).toBe(false);
  });

  it('restores all participants to the previous device after a partial rejection and persists that selection', async () => {
    const one = audio();
    const two = audio();
    vi.mocked(two.setSinkId).mockImplementation(async (id) => { if (id === 'missing') throw new Error('Device missing'); });
    renderHook(() => useRemoteAudio());
    act(() => useVoiceStore.getState().setOutputDevice('working'));
    await waitFor(() => expect(two.setSinkId).toHaveBeenLastCalledWith('working'));
    act(() => useVoiceStore.getState().setOutputDevice('missing'));
    await waitFor(() => expect(useVoiceStore.getState().outputDeviceError).toMatch(/Device missing.*previous output device/));
    expect(useVoiceStore.getState().outputDeviceId).toBe('working');
    expect(one.setSinkId).toHaveBeenLastCalledWith('working');
    expect(JSON.parse(localStorage.getItem('annex:audioSettings')!).outputDeviceId).toBe('working');
  });

  it('falls back to the default when a saved device is unavailable to a new participant', async () => {
    useVoiceStore.setState({ outputDeviceId: 'unplugged' });
    renderHook(() => useRemoteAudio());
    const call = document.createElement('audio');
    call.dataset.webrtcRemote = '';
    call.setSinkId = vi.fn(async (id) => { if (id) throw new Error('Not found'); });
    await act(async () => { document.body.append(call); });
    await waitFor(() => expect(useVoiceStore.getState().outputDeviceId).toBeNull());
    expect(call.setSinkId).toHaveBeenLastCalledWith('');
    expect(useVoiceStore.getState().outputDeviceError).toMatch(/system default/);
  });

  it('serializes a delayed old selection ahead of the newest choice without a stale rollback', async () => {
    const call = audio();
    let rejectOld!: (error: Error) => void;
    vi.mocked(call.setSinkId).mockImplementation((id) => id === 'old' ? new Promise<void>((_, reject) => { rejectOld = reject; }) : Promise.resolve());
    renderHook(() => useRemoteAudio());
    act(() => useVoiceStore.getState().setOutputDevice('old'));
    await waitFor(() => expect(rejectOld).toBeDefined());
    act(() => useVoiceStore.getState().setOutputDevice('new'));
    await act(async () => { rejectOld(new Error('Old selection refused')); });
    await waitFor(() => expect(call.setSinkId).toHaveBeenLastCalledWith('new'));
    expect(useVoiceStore.getState().outputDeviceId).toBe('new');
    expect(useVoiceStore.getState().outputDeviceError).toBeNull();
  });

  it('reports when even the default output cannot be restored', async () => {
    const call = audio();
    call.setSinkId = vi.fn(async () => { throw new Error('Audio disconnected'); });
    renderHook(() => useRemoteAudio());
    await waitFor(() => expect(useVoiceStore.getState().outputDeviceError).toMatch(/could not be restored/));
  });

  it('does not change a later session when an operation rejects after unmount', async () => {
    const call = audio();
    let reject!: (error: Error) => void;
    call.setSinkId = vi.fn(() => new Promise<void>((_, r) => { reject = r; }));
    const hook = renderHook(() => useRemoteAudio());
    await waitFor(() => expect(reject).toBeDefined());
    hook.unmount();
    useVoiceStore.getState().setOutputDevice('next-session');
    await act(async () => { reject(new Error('Late failure')); });
    expect(useVoiceStore.getState().outputDeviceId).toBe('next-session');
    expect(useVoiceStore.getState().outputDeviceError).toBeNull();
  });
});
