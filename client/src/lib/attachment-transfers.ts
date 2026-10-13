import type { WsReceiveFrame } from '@/types';
import { getCredentialContext } from '@/api/core';

export interface AttachmentTransfer { status: 'pending' | 'ready' | 'failed' | 'removed'; transferredBytes: number; totalBytes: number }
const transfers = new Map<string, AttachmentTransfer>();
const listeners = new Set<() => void>();
function key(channel: string, attachment: string) { return JSON.stringify([getCredentialContext().serial, channel, attachment]); }
export function receiveAttachmentTransfer(frame: WsReceiveFrame): void {
  if (!frame.channelId || !frame.attachmentId || !['pending','ready','failed','removed'].includes(frame.status ?? '')
    || !Number.isSafeInteger(frame.transferredBytes) || !Number.isSafeInteger(frame.totalBytes)
    || frame.transferredBytes! < 0 || frame.totalBytes! < frame.transferredBytes!) return;
  transfers.set(key(frame.channelId, frame.attachmentId), { status: frame.status!, transferredBytes: frame.transferredBytes!, totalBytes: frame.totalBytes! });
  if (transfers.size > 2000) transfers.delete(transfers.keys().next().value!);
  for (const listener of listeners) listener();
}
export function getAttachmentTransfer(channel: string, attachment: string): AttachmentTransfer | undefined { return transfers.get(key(channel, attachment)); }
export function subscribeAttachmentTransfers(listener: () => void): () => void { listeners.add(listener); return () => { listeners.delete(listener); }; }
