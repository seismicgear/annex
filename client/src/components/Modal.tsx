/**
 * The app's modal dialog primitive.
 *
 * Every dialog previously hand-rolled the same two divs:
 *
 *   <div className="dialog-overlay" onClick={onClose}>
 *     <div className="dialog" onClick={(e) => e.stopPropagation()}>
 *
 * which gave click-outside-to-close and nothing else. The UI audit found the
 * consequences on every one of them: no `role="dialog"`, so assistive tech
 * never announced them as modal; focus left where it was, so a keyboard user
 * had to tab through the page behind to reach the dialog; Tab walked straight
 * back out of it; and Escape did nothing. That was 212 findings across the
 * captured surfaces — by a wide margin the largest single accessibility gap in
 * the app, and all of it one missing primitive.
 *
 * This component owns that contract so no dialog has to remember it:
 *
 *   - `role="dialog"` + `aria-modal` + a label wired to the heading.
 *   - Focus moves inside on open and returns to the trigger on close, so
 *     dismissing a dialog does not dump the user at the top of the page.
 *   - Tab and Shift+Tab cycle within the dialog.
 *   - Escape closes.
 *   - The page behind is inert to assistive tech while the dialog is open.
 *
 * Markup and class names are unchanged, so existing styles and the committed
 * screenshot baselines still apply.
 */

import {
  useCallback,
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";

/**
 * Elements that can hold focus. `[tabindex="-1"]` is deliberately excluded:
 * it means "focusable programmatically but not in the tab order", so
 * including it would make the trap cycle through things Tab should skip.
 */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  'input:not([disabled]):not([type="hidden"])',
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export interface ModalProps {
  onClose: () => void;
  children: ReactNode;
  /** Extra classes on the dialog surface, e.g. `settings-dialog`. */
  className?: string;
  /**
   * Extra classes on the backdrop.
   *
   * Only needed by dialogs whose backdrop differs from the standard scrim —
   * the image lightbox wants a darker one and a much higher stacking order,
   * and before this it hand-rolled the whole overlay to get them, which cost
   * it every behaviour this component provides.
   */
  overlayClassName?: string;
  /**
   * Accessible name. Prefer `titleId` when the dialog already renders a
   * heading — a visible heading and its accessible name should not drift.
   */
  label?: string;
  /** Id of the heading that names this dialog. */
  titleId?: string;
  /**
   * Changing this re-runs the focus-in step.
   *
   * Several dialogs are multi-step — DeviceLinkDialog swaps between
   * choose/share/receive, SocialRecoveryDialog between four modes — and
   * replace their entire contents without unmounting. Focusing only on mount
   * left the user on a control that no longer exists after a step change, so
   * the next Tab started from the top of the document. Pass the current step
   * here and focus follows the content.
   */
  focusKey?: string | number;
}

export function Modal({
  onClose,
  children,
  className = "",
  overlayClassName = "",
  label,
  titleId,
  focusKey,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const restoreFocusTo = useRef<HTMLElement | null>(null);
  /** True between mount and unmount, so a `focusKey` change is not mistaken for a close. */
  const mountedRef = useRef(true);

  // Keep the latest onClose without re-running the mount effect, so a caller
  // passing an inline arrow does not tear down and rebuild focus handling on
  // every render. Synced in an effect rather than during render — writing a
  // ref while rendering is unsafe under concurrent React.
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => () => { mountedRef.current = false; }, []);

  useEffect(() => {
    // Remember the trigger once; a step change must not overwrite it with a
    // control inside the dialog.
    restoreFocusTo.current ??= document.activeElement as HTMLElement | null;

    const dialog = dialogRef.current;
    if (dialog) {
      // Prefer the first genuinely focusable control; fall back to the dialog
      // itself so focus is at least inside rather than left behind it.
      const first = dialog.querySelector<HTMLElement>(FOCUSABLE);
      (first ?? dialog).focus({ preventScroll: true });
    }

    return () => {
      // Returning focus to the trigger is what makes a dialog feel like a
      // detour rather than a navigation. Only on unmount — a step change
      // within the dialog must not throw focus back to the page.
      if (!mountedRef.current) restoreFocusTo.current?.focus?.({ preventScroll: true });
    };
  }, [focusKey]);

  // Escape closes, from anywhere — including from inside an input, which is
  // where a user is most likely to want out.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onCloseRef.current();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  /**
   * Pull focus back when it escapes to nowhere.
   *
   * The Tab trap below is a `keydown` handler on the dialog, so it only runs
   * while focus is already inside. That leaves one hole, and it is the common
   * case rather than an edge case: nearly every dialog here disables its
   * submit button while the request is in flight
   * (`disabled={submitting || ...}`), and a browser blurs an element the
   * moment it becomes disabled. Focus lands on `document.body`, outside the
   * dialog, where the trap can no longer see it — so a keyboard user who
   * submits a dialog and gets an error back is silently dumped behind the
   * modal, and the next Tab walks the page underneath. The UI audit caught
   * exactly this on `storage-gate-507`.
   *
   * The same thing happens whenever a focused control is conditionally
   * unmounted mid-interaction, which is why this belongs in the primitive
   * rather than in each dialog.
   *
   * This is a NATIVE `focusout` listener rather than React's `onBlur`
   * deliberately: React suppresses synthetic events originating from disabled
   * form controls, which is precisely the case this exists to catch. A focus
   * trap should not depend on the framework's event filtering.
   */
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    const restore = (departed: HTMLElement | null) => {
      if (!mountedRef.current) return;
      const current = dialogRef.current;
      if (!current) return;
      if (current.contains(document.activeElement)) return;
      // Focus genuinely left the dialog while it is still open. The control
      // that held it is usually about to come back — a submit button
      // re-enabling once the request settles — so prefer returning focus
      // exactly where it was, and fall back to the first control only if it is
      // really gone.
      if (departed?.isConnected && !departed.hasAttribute("disabled")) {
        departed.focus({ preventScroll: true });
        return;
      }
      const first = current.querySelector<HTMLElement>(FOCUSABLE);
      (first ?? current).focus({ preventScroll: true });
    };

    const onFocusOut = (e: FocusEvent) => {
      const next = e.relatedTarget as Node | null;
      // Focus moved somewhere real and still inside: nothing to do.
      if (next && dialog.contains(next)) return;
      const departed = e.target as HTMLElement | null;
      // Re-check once React has committed, so a control that is coming back
      // has come back before we decide where to put focus.
      queueMicrotask(() => restore(departed));
    };

    dialog.addEventListener("focusout", onFocusOut);
    return () => dialog.removeEventListener("focusout", onFocusOut);
  }, []);

  const handleKeyDown = useCallback((e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Tab") return;
    const dialog = dialogRef.current;
    if (!dialog) return;

    const focusable = [
      ...dialog.querySelectorAll<HTMLElement>(FOCUSABLE),
    ].filter((el) =>
      // Keep hidden branches of a multi-step dialog out of the cycle.
      //
      // `checkVisibility()` is the correct check but needs a layout engine, so
      // it is absent under jsdom. Falling back to "include it" there is the
      // right default: a test environment with no layout has no hidden
      // branches to exclude, and excluding everything would silently disable
      // the trap in exactly the place it is being tested.
      typeof el.checkVisibility === "function" ? el.checkVisibility() : true,
    );
    if (focusable.length === 0) {
      // Nothing to cycle between; keep focus on the dialog rather than
      // letting Tab escape to the page behind.
      e.preventDefault();
      return;
    }

    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement as HTMLElement | null;

    if (e.shiftKey && (active === first || active === dialog)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  }, []);

  return (
    <div className={`dialog-overlay ${overlayClassName}`.trim()} onClick={onClose}>
      <div
        ref={dialogRef}
        className={`dialog ${className}`.trim()}
        role="dialog"
        aria-modal="true"
        {...(titleId
          ? { "aria-labelledby": titleId }
          : { "aria-label": label ?? "Dialog" })}
        // Focusable so the dialog can hold focus itself when it contains no
        // controls; -1 keeps it out of the tab order.
        tabIndex={-1}
        onKeyDown={handleKeyDown}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
