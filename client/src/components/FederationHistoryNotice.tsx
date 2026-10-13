import { useEffect, useState } from 'react';
import { getFederationGaps, type FederationHistoryGap } from '@/api/federation';
import { useChannelsStore } from '@/stores/channels';

/** Persisted server receipts survive reload; live notices update an open room. */
export function FederationHistoryNotice({ channelId, pseudonymId }: { channelId: string; pseudonymId: string }) {
  const ws = useChannelsStore((s) => s.ws);
  const connected = useChannelsStore((s) => s.wsConnected);
  const [gaps, setGaps] = useState<FederationHistoryGap[]>([]);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const merge = (incoming: FederationHistoryGap[]) => {
      if (cancelled) return;
      setGaps((existing) => {
        const byOrigin = new Map(existing.map((gap) => [gap.originating_server, gap.through_seq]));
        for (const gap of incoming) {
          byOrigin.set(gap.originating_server, Math.max(byOrigin.get(gap.originating_server) ?? 0, gap.through_seq));
        }
        return [...byOrigin].map(([originating_server, through_seq]) => ({ originating_server, through_seq }));
      });
    };
    const unsubscribe = ws?.onMessage((frame) => {
      if (frame.type === 'federation_history_pruned' && frame.channelId === channelId &&
          typeof frame.originatingServer === 'string' && typeof frame.throughSeq === 'number' &&
          Number.isSafeInteger(frame.throughSeq) && frame.throughSeq >= 0) {
        merge([{ originating_server: frame.originatingServer, through_seq: frame.throughSeq }]);
      }
    });
    getFederationGaps(pseudonymId, channelId).then((incoming) => {
      merge(incoming);
      if (!cancelled) setError(false);
    }).catch(() => {
      if (!cancelled) setError(true);
    });
    return () => { cancelled = true; unsubscribe?.(); };
  }, [channelId, pseudonymId, ws, connected, attempt]);

  return <>
    {gaps.length > 0 && <div className="scrollback-error" role="status">
      <span>Some federated messages are unavailable because their origin removed retained history.</span>
      <ul>{gaps.map((gap) => <li key={gap.originating_server}>{gap.originating_server} — through sequence {gap.through_seq}</li>)}</ul>
    </div>}
    {error && <div className="scrollback-error" role="alert">
      <span>Could not check for gaps in federated history.</span>
      <button type="button" onClick={() => { setError(false); setAttempt((value) => value + 1); }}>Retry history check</button>
    </div>}
  </>;
}
