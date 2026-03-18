import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import { SocialRecoveryDialog } from './SocialRecoveryDialog';

const mockSplitSecretKey = vi.fn();
vi.mock('@/lib/shamir', () => ({
  splitSecretKey: (...args: unknown[]) => mockSplitSecretKey(...args),
  reconstructSecretKey: vi.fn(),
}));

vi.mock('@/lib/zk', () => ({
  initPoseidon: vi.fn(async () => {}),
  generateNodeId: vi.fn(() => '1234'),
  computeCommitment: vi.fn(async () => 'abcd'),
}));

const mockIdentity = {
  id: 'id-1',
  sk: 'deadbeef',
  pseudonymId: 'pseudo-123456789012',
};

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: any) => unknown) => selector({
    identity: mockIdentity,
    importBackup: vi.fn(async () => {}),
  }),
}));

describe('SocialRecoveryDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Default: split returns 3 shards
    mockSplitSecretKey.mockReturnValue([
      { index: 1, data: 'shard1data000000000000000000000000000' },
      { index: 2, data: 'shard2data000000000000000000000000000' },
      { index: 3, data: 'shard3data000000000000000000000000000' },
    ]);
  });

  it('shows inline fallback field when clipboard write is denied', async () => {
    // Mock clipboard to reject
    const originalClipboard = navigator.clipboard;
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockRejectedValue(new DOMException('Denied', 'NotAllowedError')) },
      writable: true,
      configurable: true,
    });

    const onClose = vi.fn();
    render(<SocialRecoveryDialog onClose={onClose} />);

    // Navigate to Setup
    fireEvent.click(screen.getByText('Set Up Recovery'));

    // Set totalShards to 3 (matches the 3 default guardian slots)
    const totalInput = screen.getByLabelText(/Total Guardians/);
    fireEvent.change(totalInput, { target: { value: '3' } });

    // Fill in guardian names
    const guardianInputs = screen.getAllByPlaceholderText(/Guardian \d+ name/);
    for (let i = 0; i < guardianInputs.length; i++) {
      fireEvent.change(guardianInputs[i], { target: { value: `Guardian ${i + 1}` } });
    }

    // Generate shards
    fireEvent.click(screen.getByText('Generate Shards'));

    await waitFor(() => {
      expect(screen.getByText(/Shard #1/)).toBeInTheDocument();
    });

    // Click Copy on the first shard
    const copyBtns = screen.getAllByText('Copy');
    await act(async () => {
      fireEvent.click(copyBtns[0]);
    });

    // Should show the fallback text field with the shard JSON
    await waitFor(() => {
      expect(screen.getByText(/Clipboard access denied/)).toBeInTheDocument();
    });
    const fallbackInput = screen.getByDisplayValue(/"index":1/);
    expect(fallbackInput).toBeInTheDocument();
    expect(fallbackInput).toHaveAttribute('readOnly');

    // Restore clipboard
    Object.defineProperty(navigator, 'clipboard', {
      value: originalClipboard,
      writable: true,
      configurable: true,
    });
  });

  it('shows Copied! on successful clipboard write', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      writable: true,
      configurable: true,
    });

    const onClose = vi.fn();
    render(<SocialRecoveryDialog onClose={onClose} />);

    fireEvent.click(screen.getByText('Set Up Recovery'));

    const totalInput2 = screen.getByLabelText(/Total Guardians/);
    fireEvent.change(totalInput2, { target: { value: '3' } });

    const guardianInputs = screen.getAllByPlaceholderText(/Guardian \d+ name/);
    for (let i = 0; i < guardianInputs.length; i++) {
      fireEvent.change(guardianInputs[i], { target: { value: `Guardian ${i + 1}` } });
    }

    fireEvent.click(screen.getByText('Generate Shards'));

    await waitFor(() => {
      expect(screen.getByText(/Shard #1/)).toBeInTheDocument();
    });

    const copyBtns = screen.getAllByText('Copy');
    await act(async () => {
      fireEvent.click(copyBtns[0]);
    });

    await waitFor(() => {
      expect(screen.getByText('Copied!')).toBeInTheDocument();
    });

    // No fallback field should appear
    expect(screen.queryByText(/Clipboard access denied/)).not.toBeInTheDocument();
  });
});
