/**
 * Re-render when the attachment grant changes.
 *
 * Chat attachments are fetched with a short-lived signed grant appended by
 * `resolveUrl`. That function is called during render and cannot await, so the
 * first paint after a cold start emits an unsigned URL — and without a
 * subscription, nothing would ever render it again. The image stays broken for
 * the life of the view.
 *
 * The UI audit is what found this: `/api/uploads/grant` succeeded and the image
 * requests kept coming back 401, on exactly the surface whose attachments paint
 * immediately.
 *
 * `useSyncExternalStore` rather than `useState` plus an effect: the grant lives
 * outside React, in a module cache shared by every consumer, which is precisely
 * what this hook exists for.
 */
import { useSyncExternalStore } from 'react';
import { subscribeUploadGrant, getUploadGrantVersion } from '@/lib/api';

/** A version number that changes whenever the cached grant does. */
export function useUploadGrant(): number {
  return useSyncExternalStore(subscribeUploadGrant, getUploadGrantVersion, getUploadGrantVersion);
}
