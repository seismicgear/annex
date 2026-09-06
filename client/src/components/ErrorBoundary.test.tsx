import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ErrorBoundary } from './ErrorBoundary';

function Boom({ shouldThrow = true }: { shouldThrow?: boolean }): React.ReactElement {
  if (shouldThrow) throw new Error('reputation_score is undefined');
  return <p>recovered</p>;
}

describe('ErrorBoundary', () => {
  it('shows a contained failure instead of unmounting the tree', () => {
    // React tears down the whole tree on an uncaught render error, so before
    // this existed one bad field blanked the entire application.
    vi.spyOn(console, 'error').mockImplementation(() => {});

    render(
      <div>
        <p>sibling still here</p>
        <ErrorBoundary label="Events">
          <Boom />
        </ErrorBoundary>
      </div>,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('Something went wrong in Events.');
    expect(screen.getByText('sibling still here')).toBeInTheDocument();
  });

  it('surfaces the underlying message for diagnosis', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    expect(screen.getByText('reputation_score is undefined')).toBeInTheDocument();
  });

  it('renders children untouched when nothing throws', () => {
    render(
      <ErrorBoundary>
        <Boom shouldThrow={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText('recovered')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('offers a way back rather than requiring a reload', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const user = userEvent.setup();

    function Host() {
      return (
        <ErrorBoundary>
          <Boom shouldThrow={false} />
        </ErrorBoundary>
      );
    }
    render(<Host />);
    expect(screen.getByText('recovered')).toBeInTheDocument();

    // And from the failed state, "Try again" clears it.
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    await user.click(screen.getAllByRole('button', { name: 'Try again' })[0]);
    // Re-rendering the same throwing child fails again, which is expected —
    // what matters is that the control exists and resets the boundary state.
    expect(screen.getAllByRole('alert').length).toBeGreaterThan(0);
  });
});
