import { useState } from 'react';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ErrorBoundary } from './ErrorBoundary';

// This is explicitly a component fault test, not a real-server/browser receipt.
it('preserves a working sibling and successfully retries a child once its render fault is repaired', async () => {
  const logging = vi.spyOn(console, 'error').mockImplementation(() => {});
  let unavailable = true;
  function AffectedPanel() {
    if (unavailable) throw new Error('Controlled component dependency is unavailable');
    return <p>Panel recovered</p>;
  }
  function IndependentPanel() {
    const [count, setCount] = useState(0);
    return <button onClick={() => setCount(count + 1)}>Independent actions: {count}</button>;
  }
  try {
    const user = userEvent.setup();
    render(<><IndependentPanel /><ErrorBoundary label="Affected panel"><AffectedPanel /></ErrorBoundary></>);
    const failure = screen.getByRole('alert');
    expect(failure).toHaveTextContent('Something went wrong in Affected panel.');
    await user.click(within(failure).getByText('Details'));
    expect(failure.querySelector('details')).toHaveAttribute('open');
    expect(within(failure).getByText('Controlled component dependency is unavailable')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Independent actions: 0' }));
    expect(screen.getByRole('button', { name: 'Independent actions: 1' })).toBeVisible();
    unavailable = false;
    await user.click(within(failure).getByRole('button', { name: 'Try again' }));
    expect(screen.getByText('Panel recovered')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Independent actions: 1' })).toBeVisible();
    expect(logging).toHaveBeenCalled();
  } finally { logging.mockRestore(); }
});
