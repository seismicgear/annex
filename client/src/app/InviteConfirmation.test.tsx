/**
 * The `annex://` invite banner, which existed in four byte-identical copies —
 * one per startup gate and one in the main layout. The copies are gone; these
 * pin what the single one has to do.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { InviteConfirmation } from './InviteConfirmation';

const invite = { server: 'https://peer.example', code: 'abc123' } as never;

describe('InviteConfirmation', () => {
  afterEach(() => cleanup());

  it('names the server the invite is for', () => {
    render(<InviteConfirmation invite={invite} onAccept={vi.fn()} onIgnore={vi.fn()} />);

    // The server is the whole decision: accepting registers an identity with
    // a host an attacker may have chosen.
    expect(screen.getByText(/https:\/\/peer\.example/)).toBeInTheDocument();
  });

  it('renders nothing at all without an invite', () => {
    // Callers relied on repeating `{invite && (...)}` around each copy. One of
    // them forgetting would have put an empty banner on screen.
    const { container } = render(
      <InviteConfirmation invite={null} onAccept={vi.fn()} onIgnore={vi.fn()} />,
    );

    expect(container).toBeEmptyDOMElement();
  });

  it('is a labelled region, not a dialog', () => {
    // It sits in normal flow with no overlay, no focus moved into it, no trap
    // and no Escape. Claiming `dialog` told assistive tech it was modal when
    // nothing about it behaved that way.
    render(<InviteConfirmation invite={invite} onAccept={vi.fn()} onIgnore={vi.fn()} />);

    expect(screen.getByRole('region', { name: 'Invite confirmation' })).toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('offers both answers, and calls only the one pressed', async () => {
    const onAccept = vi.fn();
    const onIgnore = vi.fn();
    const user = userEvent.setup();
    render(<InviteConfirmation invite={invite} onAccept={onAccept} onIgnore={onIgnore} />);

    await user.click(screen.getByRole('button', { name: 'Ignore' }));
    expect(onIgnore).toHaveBeenCalledTimes(1);
    expect(onAccept).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Join invite server' }));
    expect(onAccept).toHaveBeenCalledTimes(1);
    expect(onIgnore).toHaveBeenCalledTimes(1);
  });
});
