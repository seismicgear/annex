import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { IdentitySetup } from './IdentitySetup';
import { useIdentityStore } from '@/stores/identity';

describe('recovery without an existing local identity', () => {
  afterEach(cleanup);
  it('opens guardian reconstruction before any server registration or key generation', () => {
    const generate = vi.fn();
    useIdentityStore.setState({ identity: null, phase: 'uninitialized', storedIdentities: [], error: null, generateLocalKeys: generate });
    render(<IdentitySetup />);
    fireEvent.click(screen.getByRole('button', { name: 'Recover Identity' }));
    expect(screen.getByRole('button', { name: 'Reconstruct Key' })).toBeInTheDocument();
    expect(screen.getByLabelText('Shard 1, as sent by your guardian')).toBeInTheDocument();
    expect(generate).not.toHaveBeenCalled();
    expect(useIdentityStore.getState().identity).toBeNull();
  });
});
