import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Message, WsReceiveFrame } from '@/types';
import { useChannelsStore } from './channels';
import { isChannelE2eUnknown, isChannelE2e, markChannelE2e } from '@/lib/message-crypto';

const state = vi.hoisted(() => ({ receive: null as ((frame: WsReceiveFrame) => void) | null,
  send: vi.fn(() => 'request'), subscribe: vi.fn(), unsubscribe: vi.fn(),
  join: vi.fn(), leave: vi.fn(), history: vi.fn(), policy: vi.fn(), ready: vi.fn(), reset: vi.fn(), clearCall: vi.fn(),
}));
vi.mock('@/lib/api', () => ({ joinChannel: (...args: unknown[]) => state.join(...args), leaveChannel: (...args: unknown[]) => state.leave(...args),
  getMessages: (...args: unknown[]) => state.history(...args), getChannelE2e: (...args: unknown[]) => state.policy(...args) }));
vi.mock('@/lib/ws', () => ({ AnnexWebSocket: class {
  connected = true; onStatus() {} connect() {} disconnect() {} trackLastMessageId() {}
  subscribe = state.subscribe; unsubscribe = state.unsubscribe; send = state.send;
  onMessage(handler: (frame: WsReceiveFrame) => void) { state.receive = handler; }
} }));
vi.mock('@/lib/message-crypto', async original => ({ ...await original<typeof import('@/lib/message-crypto')>(), ensureChannelReady: () => state.ready() }));
vi.mock('@/stores/voice', () => ({ useVoiceStore: { getState: () => ({ connectedChannelId: 'room', forceReset: state.reset, clearChannelCallState: state.clearCall }) } }));
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { resolve, promise }; }
const message: Message = { message_id: 'saved', channel_id: 'room', sender_pseudonym: 'alice', content: 'stored', created_at: '', reply_to_message_id: null };
beforeEach(() => {
  useChannelsStore.getState().resetServerState(); vi.clearAllMocks();
  state.join.mockResolvedValue(undefined); state.leave.mockResolvedValue(undefined); state.history.mockResolvedValue([]); state.policy.mockResolvedValue(false); state.ready.mockResolvedValue(undefined);
  useChannelsStore.setState({ channels: [{ channel_id: 'room', name: 'Room', channel_type: 'Text', topic: null, federation_scope: 'Local', is_member: true }], joinedChannelIds: new Set(['room', 'plain']) });
  useChannelsStore.getState().connectWs('alice');
  state.subscribe.mockClear();
});
afterEach(() => useChannelsStore.getState().disconnectWs());
const remove = () => state.receive!({ type: 'channel_membership_changed', channelId: 'room', pseudonymId: 'alice', member: false });

describe('signed membership and verified selection lifecycle', () => {
  it('blocks the fastest send while status is unknown and allows a verified ordinary channel afterward', async () => {
    const gate = deferred<boolean>(); state.policy.mockReturnValueOnce(gate.promise);
    const opening = useChannelsStore.getState().selectChannel('alice', 'room');
    expect(isChannelE2eUnknown('room')).toBe(true);
    expect(useChannelsStore.getState().sendMessage('must not leak', 'alice')).toBeNull();
    expect(state.send).not.toHaveBeenCalled();
    gate.resolve(true); await opening; expect(isChannelE2e('room')).toBe(true);
    await useChannelsStore.getState().selectChannel('alice', 'plain');
    expect(isChannelE2eUnknown('plain')).toBe(false); expect(isChannelE2e('plain')).toBe(false);
    expect(useChannelsStore.getState().sendMessage('ordinary', 'alice')).toBe('request');
    expect(state.send).toHaveBeenCalledWith('plain', 'ordinary', null);
  });
  it('does not let a stale plaintext selection response undo a newer verified policy hint', async () => {
    const old = deferred<boolean>(); state.policy.mockReturnValueOnce(old.promise).mockResolvedValueOnce(true);
    const opening = useChannelsStore.getState().selectChannel('alice', 'room');
    state.receive!({ type: 'channel_e2e_changed', channelId: 'room', e2eEnabled: true });
    await vi.waitFor(() => expect(isChannelE2e('room')).toBe(true));
    old.resolve(false); await opening;
    expect(isChannelE2e('room')).toBe(true); expect(useChannelsStore.getState().activeChannelE2e).toBe(true);
  });
  it('keeps writes blocked across an obsolete refresh until the newer response arrives', async () => {
    useChannelsStore.setState({ activeChannelId: 'room' }); markChannelE2e('room', true);
    const first = deferred<boolean>(), latest = deferred<boolean>(); state.policy.mockReturnValueOnce(first.promise).mockReturnValueOnce(latest.promise);
    const pending = useChannelsStore.getState().refreshChannelEncryption('alice', 'room');
    await vi.waitFor(() => expect(state.policy).toHaveBeenCalledOnce());
    state.receive!({ type: 'channel_e2e_changed', channelId: 'room', e2eEnabled: true });
    first.resolve(false);
    await vi.waitFor(() => expect(state.policy).toHaveBeenCalledTimes(2));
    expect(isChannelE2eUnknown('room')).toBe(true);
    expect(useChannelsStore.getState().sendMessage('still blocked', 'alice')).toBeNull();
    latest.resolve(true); await pending; expect(isChannelE2e('room')).toBe(true);
  });
  it('closes a removed membership, capture and pending writes without rejoining or applying stale history', async () => {
    const gate = deferred<Message[]>(); state.history.mockReturnValueOnce(gate.promise);
    const opening = useChannelsStore.getState().selectChannel('alice', 'room');
    await vi.waitFor(() => expect(state.history).toHaveBeenCalledOnce());
    useChannelsStore.setState({ messages: [message], pendingSends: new Map([['pending', { channelId: 'room', clientRequestId: 'pending', content: 'unsent', sentAt: 1 }]]) });
    remove(); gate.resolve([message]); await opening;
    expect(useChannelsStore.getState()).toMatchObject({ activeChannelId: null, messages: [], historyLoading: false });
    expect(useChannelsStore.getState().joinedChannelIds.has('room')).toBe(false);
    expect(useChannelsStore.getState().pendingSends.size).toBe(0);
    expect(state.reset).toHaveBeenCalledOnce(); expect(state.unsubscribe).toHaveBeenCalledWith('room');
    expect(state.join).not.toHaveBeenCalled(); expect(state.leave).not.toHaveBeenCalled(); expect(state.subscribe).not.toHaveBeenCalled();
  });
  it('compensates an already transmitted join after removal but allows a later explicit join', async () => {
    useChannelsStore.setState({ joinedChannelIds: new Set() });
    const gate = deferred<void>(); const mutations: string[] = [];
    state.join.mockImplementationOnce(async () => { mutations.push('start'); await gate.promise; mutations.push('join'); });
    state.leave.mockImplementationOnce(async () => { mutations.push('leave'); });
    const opening = useChannelsStore.getState().selectChannel('alice', 'room');
    await vi.waitFor(() => expect(state.join).toHaveBeenCalledOnce()); remove(); gate.resolve(); await opening;
    expect(mutations).toEqual(['start', 'join', 'leave']);
    expect(useChannelsStore.getState().joinedChannelIds.has('room')).toBe(false);
    await useChannelsStore.getState().joinChannel('alice', 'room');
    expect(useChannelsStore.getState().joinedChannelIds.has('room')).toBe(true);
  });
});
