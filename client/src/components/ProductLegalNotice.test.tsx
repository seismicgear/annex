import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProductLegalNotice } from './ProductLegalNotice';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe('offline product notice', () => {
  it('opens without an account or network request, supports keyboard close and restores focus', async () => {
    const fetch = vi.fn(() => Promise.reject(new Error('offline')));
    vi.stubGlobal('fetch', fetch);
    const user = userEvent.setup();
    render(<ProductLegalNotice />);
    const trigger = screen.getByRole('button', { name: 'Privacy & license' });
    await user.click(trigger);
    const dialog = screen.getByRole('dialog', { name: 'Annex product and privacy notice' });
    expect(dialog).toHaveTextContent('Kuykendall Industries, LLC');
    expect(dialog).toHaveTextContent('An admitted agent may receive channel content');
    expect(dialog).toHaveTextContent('Signing out, disconnecting a server or revoking access');
    expect(within(dialog).getByRole('link', { name: 'Privacy / Data Request' })).toHaveAttribute('href', 'https://kuykendall.industries/contact?category=Privacy%20%2F%20Data%20Request');
    expect(fetch).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();
  });
});
