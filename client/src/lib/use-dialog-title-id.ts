import { useId } from 'react';

/**
 * Stable id for a dialog heading, for wiring `aria-labelledby`.
 *
 * Lives outside `Modal.tsx` so that file exports only components, which is
 * what React Fast Refresh needs to hot-reload it reliably.
 */
export function useDialogTitleId(): string {
  return `dialog-title-${useId()}`;
}
