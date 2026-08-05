/**
 * The modal keyboard/ARIA contract.
 *
 * Each of these pins one of the four failures the UI audit reported on every
 * hand-rolled dialog in the app — 212 findings in total, all from the same
 * missing primitive.
 */

import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
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
