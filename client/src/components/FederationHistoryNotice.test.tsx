import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { FederationHistoryNotice } from './FederationHistoryNotice';
import { useChannelsStore } from '@/stores/channels';
import type { WsReceiveFrame } from '@/types';

const getGaps = vi.hoisted(() => vi.fn());
vi.mock('@/api/federation', () => ({ getFederationGaps: (...args: unknown[]) => getGaps(...args) }));

let receive: (frame: WsReceiveFrame) => void;
const unsubscribe = vi.fn();
beforeEach(() => {
  getGaps.mockReset();
  unsubscribe.mockClear();
  useChannelsStore.setState({ ws: { onMessage: (handler: typeof receive) => { receive = handler; return unsubscribe; } } as never, wsConnected: true });
});
afterEach(cleanup);

describe('federated history loss notice', () => {
  it('shows persisted loss after reload and retains a newer live notice over an older read', async () => {
    let resolve!: (gaps: unknown[]) => void;
    getGaps.mockReturnValue(new Promise((r) => { resolve = r; }));
    render(<FederationHistoryNotice channelId="room" pseudonymId="member" />);
    act(() => receive({ type: 'federation_history_pruned', channelId: 'room', originatingServer: 'https://origin.example', throughSeq: 10 }));
    await act(async () => { resolve([{ originating_server: 'https://origin.example', through_seq: 5 }]); });
    expect(screen.getByRole('status')).toHaveTextContent('through sequence 10');
    expect(getGaps).toHaveBeenCalledWith('member', 'room');
    cleanup();
    getGaps.mockResolvedValue([{ originating_server: 'https://origin.example', through_seq: 10 }]);
    render(<FederationHistoryNotice channelId="room" pseudonymId="member" />);
    expect(await screen.findByRole('status')).toHaveTextContent('origin removed retained history');
  });

  it('ignores other channels and malformed sequences, and retries a failed read', async () => {
    getGaps.mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce([]);
    render(<FederationHistoryNotice channelId="room" pseudonymId="member" />);
    await screen.findByRole('alert');
    act(() => {
      receive({ type: 'federation_history_pruned', channelId: 'different', originatingServer: 'other', throughSeq: 5 });
      receive({ type: 'federation_history_pruned', channelId: 'room', originatingServer: 'other', throughSeq: -1 });
    });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry history check' }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(getGaps).toHaveBeenCalledTimes(2);
  });

  it('does not carry old server/channel requests into a new notice', async () => {
    let resolveOld!: (gaps: unknown[]) => void;
    getGaps.mockReturnValueOnce(new Promise((r) => { resolveOld = r; })).mockResolvedValueOnce([]);
    const view = render(<FederationHistoryNotice key="old" channelId="old" pseudonymId="member" />);
    view.rerender(<FederationHistoryNotice key="new" channelId="new" pseudonymId="member" />);
    await act(async () => { resolveOld([{ originating_server: 'old-server', through_seq: 15 }]); });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(unsubscribe).toHaveBeenCalled();
  });
});
