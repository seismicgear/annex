/** Per-message markers remain readable across channel encryption toggles. */
export const E2E_BODY_PREFIX = 'e2e1:';

export function isE2eBody(body: string): boolean {
  return typeof body === 'string' && (body.startsWith(E2E_BODY_PREFIX) || body.startsWith('e2e2:'));
}

/** An established epoch exists but this device has not been admitted to it. */
export class E2eKeyPendingError extends Error {
  constructor(channelId: string) {
    super(`E2E channel key not yet available for ${channelId} (awaiting admission)`);
    this.name = 'E2eKeyPendingError';
  }
}
