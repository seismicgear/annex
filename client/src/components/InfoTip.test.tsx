import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { InfoTip } from './InfoTip';

afterEach(cleanup);

describe('InfoTip input continuity', () => {
  it('keeps keyboard help open when the pointer leaves, until focus also leaves', async () => {
    const user = userEvent.setup();
    render(<><InfoTip text="Channel membership help" /><button>Next control</button></>);
    const trigger = screen.getByRole('button', { name: 'Channel membership help' });

    await user.tab();
    expect(trigger).toHaveFocus();
    await user.hover(trigger);
    await user.unhover(trigger);

    expect(trigger).toHaveFocus();
    expect(screen.getByRole('tooltip')).toHaveTextContent('Channel membership help');
    await user.tab();
    expect(screen.getByRole('button', { name: 'Next control' })).toHaveFocus();
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });

  it('keeps pointer help open when keyboard focus leaves, until the pointer also leaves', async () => {
    const user = userEvent.setup();
    render(<><InfoTip text="Channel membership help" /><button>Next control</button></>);
    const trigger = screen.getByRole('button', { name: 'Channel membership help' });

    await user.tab();
    await user.hover(trigger);
    await user.tab();

    expect(screen.getByRole('button', { name: 'Next control' })).toHaveFocus();
    expect(screen.getByRole('tooltip')).toHaveTextContent('Channel membership help');
    await user.unhover(trigger);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });
});
