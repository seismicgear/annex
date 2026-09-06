/**
 * The `annex://` invite banner.
 *
 * Every startup gate shows it, and so does the main layout — four
 * byte-identical copies of an eleven-line block with two handlers and an
 * aria-label, repeated verbatim. Which screen the user happens to be on when
 * a deep link arrives is incidental; the banner is one thing, and four copies
 * are four places for it to drift.
 *
 * It declares `role="region"` with `aria-live`, not `role="dialog"`. It sits
 * in normal flow with no overlay, no focus moved into it, no focus trap and
 * no Escape — calling it a dialog told assistive technology it was modal when
 * nothing about it behaved that way, and made it inconsistent with the
 * degraded-startup banner beside it, which has always been a `status`.
 *
 * Making it a genuine modal is a defensible product change — accepting an
 * `annex://` invite is consequential — but that is a decision about
 * interrupting the user, not a role attribute.
 *
 * Renders nothing without an invite, so callers do not repeat the guard.
 */

import type { InvitePayload } from '@/types';

export function InviteConfirmation({
  invite,
  onAccept,
  onIgnore,
}: {
  invite: InvitePayload | null;
  onAccept: () => void;
  onIgnore: () => void;
}) {
  if (!invite) return null;
  return (
    <div
      className="invite-confirmation-banner"
      role="region"
      aria-live="polite"
      aria-label="Invite confirmation"
    >
      <span>Invite received for {invite.server}</span>
      <button className="primary-btn" onClick={onAccept}>
        Join invite server
      </button>
      <button className="secondary-btn" onClick={onIgnore}>
        Ignore
      </button>
    </div>
  );
}
