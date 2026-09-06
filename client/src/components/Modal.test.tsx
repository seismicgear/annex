/**
 * The modal keyboard/ARIA contract.
 *
 * Each of these pins one of the four failures the UI audit reported on every
 * hand-rolled dialog in the app — 212 findings in total, all from the same
 * missing primitive.
 */

import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Modal } from './Modal';

function Fixture({ onClose = () => {} }: { onClose?: () => void }) {
  return (
    <Modal onClose={onClose} label="Test dialog">
      <button>first</button>
      <input aria-label="middle" />
      <button>last</button>
    </Modal>
  );
}

describe('Modal', () => {
  it('announces itself as a modal dialog', () => {
    render(<Fixture />);
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveAccessibleName('Test dialog');
  });

  it('names itself from its heading when given one', () => {
    render(
      <Modal onClose={() => {}} titleId="t1">
        <h3 id="t1">Create Channel</h3>
      </Modal>,
    );
    expect(screen.getByRole('dialog')).toHaveAccessibleName('Create Channel');
  });

  it('moves focus inside on open', () => {
    render(<Fixture />);
    expect(screen.getByRole('button', { name: 'first' })).toHaveFocus();
  });

  it('cycles Tab within the dialog instead of escaping it', async () => {
    const user = userEvent.setup();
    render(<Fixture />);

    await user.tab(); // first -> middle
    expect(screen.getByLabelText('middle')).toHaveFocus();
    await user.tab(); // middle -> last
    expect(screen.getByRole('button', { name: 'last' })).toHaveFocus();
    await user.tab(); // wraps back to first rather than leaving
    expect(screen.getByRole('button', { name: 'first' })).toHaveFocus();
  });

  it('cycles backwards with Shift+Tab', async () => {
    const user = userEvent.setup();
    render(<Fixture />);

    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: 'last' })).toHaveFocus();
  });

  it('closes on Escape', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Fixture onClose={onClose} />);

    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes on Escape from inside a text field', async () => {
    // Escape bound to the dialog element only would not fire while a nested
    // input has focus — which is exactly where a user wants out.
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Fixture onClose={onClose} />);

    await user.click(screen.getByLabelText('middle'));
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes on a backdrop click but not on a click inside', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { container } = render(<Fixture onClose={onClose} />);

    await user.click(screen.getByRole('button', { name: 'first' }));
    expect(onClose, 'clicks inside must not dismiss').not.toHaveBeenCalled();

    await user.click(container.querySelector('.dialog-overlay')!);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('returns focus to whatever opened it', async () => {
    // Without this, dismissing a dialog drops a keyboard user at the top of
    // the document instead of back at the control they came from.
    const user = userEvent.setup();

    function Host() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button onClick={() => setOpen(true)}>open</button>
          {open && (
            <Modal onClose={() => setOpen(false)} label="Test dialog">
              <button>inside</button>
            </Modal>
          )}
        </>
      );
    }

    render(<Host />);
    const trigger = screen.getByRole('button', { name: 'open' });
    await user.click(trigger);
    expect(screen.getByRole('button', { name: 'inside' })).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(trigger).toHaveFocus();
  });

  it('keeps focus in an empty dialog rather than letting Tab out', async () => {
    const user = userEvent.setup();
    render(
      <Modal onClose={() => {}} label="Empty">
        <p>nothing focusable here</p>
      </Modal>,
    );

    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveFocus();
    await user.tab();
    expect(dialog).toHaveFocus();
  });
});

describe('Modal focusKey', () => {
  it('moves focus to the new step when the dialog changes contents', async () => {
    // Multi-step dialogs (device linking, social recovery) replace their
    // contents without unmounting. Focusing only on mount left the user on a
    // control that no longer existed, so the next Tab started from the top of
    // the document.
    const user = userEvent.setup();

    function Stepper() {
      const [step, setStep] = useState<'one' | 'two'>('one');
      return (
        <Modal onClose={() => {}} label="Stepper" focusKey={step}>
          {step === 'one' ? (
            <button onClick={() => setStep('two')}>go to step two</button>
          ) : (
            <button>step two control</button>
          )}
        </Modal>
      );
    }

    render(<Stepper />);
    const first = screen.getByRole('button', { name: 'go to step two' });
    expect(first).toHaveFocus();

    await user.click(first);
    expect(screen.getByRole('button', { name: 'step two control' })).toHaveFocus();
  });

  it('does not return focus to the trigger on a step change', async () => {
    const user = userEvent.setup();

    function Host() {
      const [open, setOpen] = useState(false);
      const [step, setStep] = useState(0);
      return (
        <>
          <button onClick={() => setOpen(true)}>open</button>
          {open && (
            <Modal onClose={() => setOpen(false)} label="Stepper" focusKey={step}>
              <button onClick={() => setStep((s) => s + 1)}>next ({step})</button>
            </Modal>
          )}
        </>
      );
    }

    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'open' }));
    await user.click(screen.getByRole('button', { name: 'next (0)' }));

    // Focus must stay inside the dialog, not snap back to "open".
    expect(screen.getByRole('button', { name: 'next (1)' })).toHaveFocus();
  });
});

/**
 * Focus recovery when a control disappears from under the user.
 *
 * The Tab trap is a keydown handler on the dialog, so it only sees focus that
 * is already inside. Nearly every dialog in the app disables its submit button
 * while the request is in flight, and a browser blurs an element the moment it
 * becomes disabled — dropping focus to `document.body`, outside the dialog,
 * where the trap can no longer reach it. The UI audit caught this in real
 * Chromium on `storage-gate-507`, the one surface that captures a dialog
 * submit that fails: focus was neither inside the dialog nor trapped by it.
 *
 * jsdom does NOT implement blur-on-disable — a disabled element stays as
 * `document.activeElement` — so these tests fire the blur the browser would
 * fire and assert what the dialog does with it. The end-to-end proof that the
 * browser behaves this way, and that the fix holds there, is the audit's
 * `keyboard` check on that surface.
 */
describe('Modal focus recovery', () => {
  /** Fire the blur a browser emits when the focused element is disabled or removed. */
  function browserBlur(el: HTMLElement) {
    el.dispatchEvent(new FocusEvent('blur', { bubbles: false, relatedTarget: null }));
    // React listens for focusout for its bubbling onBlur.
    el.dispatchEvent(new FocusEvent('focusout', { bubbles: true, relatedTarget: null }));
  }

  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

  it('pulls focus back when the focused control is disabled mid-submit', async () => {
    render(
      <Modal onClose={() => {}} label="Submitting dialog">
        <input aria-label="name" />
        <button>Create</button>
      </Modal>,
    );

    const submit = screen.getByRole('button', { name: 'Create' });
    submit.focus();

    // What the browser does when React sets `disabled` on the focused button:
    // drop focus first, then the attribute lands. (jsdom's blur() is a no-op on
    // an already-disabled element, so the order matters here.)
    submit.blur();
    submit.setAttribute('disabled', '');
    browserBlur(submit);
    expect(document.activeElement).toBe(document.body);

    await settle();
    const dialog = screen.getByRole('dialog');
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(document.body);
  });

  it('returns focus to the same control once it is enabled again', async () => {
    render(
      <Modal onClose={() => {}} label="Submitting dialog">
        <input aria-label="name" />
        <button>Create</button>
      </Modal>,
    );

    const submit = screen.getByRole('button', { name: 'Create' });
    submit.focus();
    (document.activeElement as HTMLElement).blur();
    // The request settled and the button re-enabled before the recheck runs.
    browserBlur(submit);

    await settle();
    // Not just "somewhere inside" — back on the button the user was using, so
    // Enter retries rather than triggering whatever happens to come first.
    expect(document.activeElement).toBe(submit);
  });

  it('falls back to the first control when the focused one is gone for good', async () => {
    render(
      <Modal onClose={() => {}} label="Submitting dialog">
        <button>first</button>
        <button>doomed</button>
      </Modal>,
    );

    const doomed = screen.getByRole('button', { name: 'doomed' });
    doomed.focus();
    (document.activeElement as HTMLElement).blur();
    doomed.remove();
    browserBlur(doomed);

    await settle();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'first' }));
  });

  it('keeps Tab trapped after the recovery', async () => {
    const user = userEvent.setup();
    render(<Fixture />);

    const last = screen.getByRole('button', { name: 'last' });
    last.focus();
    (document.activeElement as HTMLElement).blur();
    browserBlur(last);
    await settle();

    const dialog = screen.getByRole('dialog');
    for (let i = 0; i < 8; i++) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
  });

  it('does not fight a deliberate move to another control inside', async () => {
    const user = userEvent.setup();
    render(<Fixture />);

    await user.click(screen.getByRole('button', { name: 'last' }));
    await settle();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'last' }));
  });
});

describe('focus does not land on a tooltip trigger', () => {
  // Both `identity-settings` and the peer-detail dialog begin with an
  // InfoTip. `FOCUSABLE` matches it — it carries `tabIndex={0}` so a keyboard
  // user can read it — so the dialog focused it on open and its popup covered
  // the dialog's own heading. Three committed baselines had recorded that as
  // the correct appearance.
  afterEach(() => cleanup());

  it('prefers a real control over an InfoTip when the dialog opens', () => {
    render(
      <Modal onClose={() => {}} label="Test">
        <span className="info-tip" tabIndex={0} role="button" aria-label="What this means" />
        <button type="button">Save</button>
      </Modal>,
    );

    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Save' }));
  });

  it('still focuses the tip when it is the only thing there', () => {
    // Skipping it is a preference, not a ban — focus must end up inside the
    // dialog either way, or the trap has nothing to work with.
    render(
      <Modal onClose={() => {}} label="Test">
        <span
          className="info-tip"
          tabIndex={0}
          role="button"
          aria-label="Only focusable thing"
        />
      </Modal>,
    );

    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: 'Only focusable thing' }),
    );
  });
});
