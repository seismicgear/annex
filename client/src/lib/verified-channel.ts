/** UI observations of independently verified history. No directory response populates this store. */
import { getCredentialContext } from '@/api/core';
import type { ManifestHistory } from './e2e-manifest-session';
import type { JoinPolicy } from './e2e-manifest';

export interface VerifiedChannelView {
  authorityIdentity: string; channelId: string; genesisHash: string | null; headHash: string | null;
  trusted: boolean; enabled: boolean; joinPolicy: JoinPolicy;
  members: { pseudonym: string; role: 'member' | 'moderator'; devices: string[] }[];
}
const views = new Map<string, VerifiedChannelView>();
const listeners = new Set<() => void>();
function key(pseudonym: string, channelId: string): string {
  return JSON.stringify([getCredentialContext().serial, pseudonym, channelId]);
}
export function observeVerifiedChannel(pseudonym: string, history: ManifestHistory): void {
  const value = history.final;
  const snapshot: VerifiedChannelView = { authorityIdentity: value.authorityIdentity, channelId: value.channelId,
    genesisHash: value.head?.genesisHash ?? null, headHash: value.head?.hash ?? null, trusted: history.trusted,
    enabled: value.enabled, joinPolicy: value.joinPolicy,
    members: [...value.members].map(([pseudonym, member]) => ({ pseudonym, role: member.role, devices: [...member.devices.keys()].sort() }))
      .sort((left, right) => left.pseudonym.localeCompare(right.pseudonym)) };
  const id = key(pseudonym, value.channelId);
  if (JSON.stringify(views.get(id)) === JSON.stringify(snapshot)) return;
  views.set(id, snapshot);
  for (const listener of listeners) listener();
}
export function getVerifiedChannel(pseudonym: string, channelId: string): VerifiedChannelView | null {
  return views.get(key(pseudonym, channelId)) ?? null;
}
export function subscribeVerifiedChannels(listener: () => void): () => void {
  listeners.add(listener); return () => { listeners.delete(listener); };
}
export function clearVerifiedChannels(): void {
  views.clear(); for (const listener of listeners) listener();
}
