import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { WsReceiveFrame } from '@/types';
import { channelKeyId, decryptMessageV2, encryptMessageV2 } from '@/lib/e2e-v2';
import { useChannelsStore } from './channels';

const bridge = vi.hoisted(() => ({ receive: null as ((frame: WsReceiveFrame) => void) | null, decrypt: vi.fn() }));
vi.mock('@/lib/ws', () => ({ AnnexWebSocket: class {
  connected = true;
  onStatus() {} connect() {} disconnect() {} subscribe() {} unsubscribe() {} trackLastMessageId() {}
  onMessage(handler: (frame: WsReceiveFrame) => void) { bridge.receive = handler; }
} }));
vi.mock('@/lib/message-crypto', async importOriginal => ({
  ...await importOriginal<typeof import('@/lib/message-crypto')>(),
  decryptForDisplay: (channelId: string, wire: string) => bridge.decrypt(channelId, wire),
}));

const key = new Uint8Array(32).fill(17);
const context = { channelId: 'room', authorityServer: 'annex-server:' + 'a'.repeat(64), epoch: 2, keyId: channelKeyId(key) };
const plaintext = 'Independently encrypted remote message';
const ciphertext = encryptMessageV2(plaintext, key, context);
function receive(values: Partial<WsReceiveFrame> = {}) {
  bridge.receive!({ type: 'message', channelId: 'room', messageId: 'message-1', senderPseudonym: 'remote',
    clientRequestId: 'remote-request', content: ciphertext, ...values });
}
beforeEach(() => {
  bridge.decrypt.mockReset().mockImplementation(async (channelId: string, wire: string) => decryptMessageV2(wire, key, { ...context, channelId }));
  useChannelsStore.setState({ activeChannelId: 'room', messages: [], joinedChannelIds: new Set(['room']), pendingSends: new Map(), pendingMessageOps: new Map() });
  useChannelsStore.getState().connectWs('local');
});
afterEach(() => { useChannelsStore.getState().disconnectWs(); });

describe('actual encrypted broadcast receive semantics', () => {
  it('decrypts another sender’s broadcast even though it carries their request ID', async () => {
    receive();
    await vi.waitFor(() => expect(useChannelsStore.getState().messages[0].content).toBe(plaintext));
    expect(bridge.decrypt).toHaveBeenCalledWith('room', ciphertext);
    expect(useChannelsStore.getState().messages[0].encrypted_content).toBe(ciphertext);
  });
  it('decrypts an own-identity replay when no local optimistic plaintext exists', async () => {
    receive({ senderPseudonym: 'local' });
    await vi.waitFor(() => expect(useChannelsStore.getState().messages[0].content).toBe(plaintext));
    expect(bridge.decrypt).toHaveBeenCalledOnce();
  });
  it('keeps the actual local pending plaintext and stable row key on its own echo', () => {
    useChannelsStore.setState({ messages: [{ message_id: '', channel_id: 'room', sender_pseudonym: 'local', content: plaintext,
      pending: true, clientRequestId: 'local-request', created_at: '', reply_to_message_id: null }],
      pendingSends: new Map([['local-request', { clientRequestId: 'local-request', content: plaintext, sentAt: Date.now() }]]) });
    receive({ senderPseudonym: 'local', clientRequestId: 'local-request' });
    expect(useChannelsStore.getState().messages).toHaveLength(1);
    expect(useChannelsStore.getState().messages[0]).toMatchObject({ message_id: 'message-1', content: plaintext, clientRequestId: 'local-request' });
    expect(useChannelsStore.getState().pendingSends.size).toBe(0);
    expect(bridge.decrypt).not.toHaveBeenCalled();
  });
  it('does not let another sender’s matching request ID confirm or replace a local send', async () => {
    useChannelsStore.setState({ messages: [{ message_id: '', channel_id: 'room', sender_pseudonym: 'local', content: 'Still pending',
      pending: true, clientRequestId: 'remote-request', created_at: '', reply_to_message_id: null }],
      pendingSends: new Map([['remote-request', { clientRequestId: 'remote-request', content: 'Still pending', sentAt: Date.now() }]]) });
    receive();
    await vi.waitFor(() => expect(useChannelsStore.getState().messages.find(message => message.message_id === 'message-1')?.content).toBe(plaintext));
    expect(useChannelsStore.getState().messages[0].content).toBe('Still pending');
    expect(useChannelsStore.getState().pendingSends.size).toBe(1);
  });
  it('retries decryption of a deduplicated encrypted replay without creating another row', async () => {
    useChannelsStore.setState({ messages: [{ message_id: 'message-1', channel_id: 'room', sender_pseudonym: 'remote',
      content: 'encrypted message (no key)', encrypted_content: ciphertext, created_at: '', reply_to_message_id: null }] });
    receive();
    await vi.waitFor(() => expect(useChannelsStore.getState().messages[0].content).toBe(plaintext));
    expect(useChannelsStore.getState().messages).toHaveLength(1);
  });
});
