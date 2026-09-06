import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import { SocialRecoveryDialog } from './SocialRecoveryDialog';

const mockSplitSecretKey = vi.fn();
const mockReconstructSecretKey = vi.fn();
vi.mock('@/lib/shamir', () => ({
  splitSecretKey: (...args: unknown[]) => mockSplitSecretKey(...args),
  reconstructSecretKey: (...args: unknown[]) => mockReconstructSecretKey(...args),
}));

const mockComputeCommitment = vi.fn(async () => 'c0ffee');
vi.mock('@/lib/zk', () => ({
  initPoseidon: vi.fn(async () => {}),
  generateNodeId: vi.fn(() => 999999),
  computeCommitment: (...a: unknown[]) => mockComputeCommitment(...(a as [])),
}));

const mockIdentity = {
  id: 'id-1',
  sk: 'deadbeef',
  roleCode: 1,
  nodeId: 482913,
  commitmentHex: 'c0ffee',
  pseudonymId: 'pseudo-123456789012',
};

const mockImportBackup = vi.fn(async () => {});

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: Record<string, unknown>) => unknown) => selector({
    identity: mockIdentity,
    importBackup: (...a: unknown[]) => mockImportBackup(...(a as [])),
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

  // ── The recovery path ────────────────────────────────────────────────
  //
  // Every test below covers a way this dialog used to report success on a
  // recovery that had not happened.

  const SHARD = (over: Record<string, unknown> = {}) =>
    JSON.stringify({
      v: 2,
      index: 1,
      data: 'aabbcc',
      threshold: 3,
      totalShards: 5,
      roleCode: 1,
      nodeId: 482913,
      commitment: 'c0ffee',
      ...over,
    });

  function openRecover() {
    render(<SocialRecoveryDialog onClose={vi.fn()} />);
    fireEvent.click(screen.getByText('Recover Identity'));
  }

  function pasteShards(blobs: string[]) {
    const fields = screen.getAllByPlaceholderText(/Paste the shard/);
    blobs.forEach((b, i) => fireEvent.change(fields[i], { target: { value: b } }));
  }

  // The shard payloads exist only in component state — the stored config holds
  // `data: '***'` on purpose — so closing this dialog destroys them. Someone
  // who closed early was left with a config saying recovery was configured and
  // no shards in existence: a safety net recorded as installed while absent.
  async function generateThreeShards(onClose: () => void) {
    render(<SocialRecoveryDialog onClose={onClose} />);
    fireEvent.click(screen.getByText('Set Up Recovery'));
    fireEvent.change(screen.getByLabelText(/Total Guardians/), { target: { value: '3' } });
    const rows = screen.getAllByPlaceholderText(/Guardian \d+ name/);
    for (let i = 0; i < rows.length; i++) {
      fireEvent.change(rows[i], { target: { value: `Guardian ${i + 1}` } });
    }
    fireEvent.click(screen.getByText('Generate Shards'));
    await waitFor(() => expect(screen.getByText(/Shard #1/)).toBeInTheDocument());
  }

  it('warns instead of closing when shards have not been copied', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      writable: true,
      configurable: true,
    });
    const onClose = vi.fn();
    await generateThreeShards(onClose);

    fireEvent.click(screen.getByText('Done'));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('alert').textContent).toMatch(/3 of 3 shards have not\s+been copied/);

    // Deliberate: the second press goes through, so the warning informs
    // rather than traps.
    fireEvent.click(screen.getByText('Done'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes on the first press once every shard is copied', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      writable: true,
      configurable: true,
    });
    const onClose = vi.fn();
    await generateThreeShards(onClose);

    // Each click takes the first button still reading exactly "Copy"; the one
    // just clicked flashes "Copied!" for two seconds and the earlier ones read
    // "Copy again", so no button is visited twice.
    for (let i = 0; i < 3; i++) {
      const next = screen.getAllByText(/^Copy$/)[0];
      await act(async () => {
        fireEvent.click(next);
      });
    }
    await waitFor(() => expect(screen.queryAllByText(/^Copy$/)).toHaveLength(0));

    fireEvent.click(screen.getByText('Done'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('offers one guardian row per shard on first open', () => {
    render(<SocialRecoveryDialog onClose={vi.fn()} />);
    fireEvent.click(screen.getByText('Set Up Recovery'));

    // Total Guardians defaults to 5. It used to render three boxes and then
    // refuse to submit, with no way to add the missing two.
    expect(Number((screen.getByLabelText(/Total Guardians/) as HTMLInputElement).value)).toBe(5);
    expect(screen.getAllByPlaceholderText(/Guardian \d+ name/)).toHaveLength(5);
  });

  it('grows and keeps the guardian rows in step with the total', () => {
    render(<SocialRecoveryDialog onClose={vi.fn()} />);
    fireEvent.click(screen.getByText('Set Up Recovery'));
    const total = screen.getByLabelText(/Total Guardians/);

    fireEvent.change(total, { target: { value: '7' } });
    expect(screen.getAllByPlaceholderText(/Guardian \d+ name/)).toHaveLength(7);

    fireEvent.change(total, { target: { value: '2' } });
    expect(screen.getAllByPlaceholderText(/Guardian \d+ name/)).toHaveLength(2);
  });

  it('fills the shard number from a pasted shard', () => {
    openRecover();
    pasteShards([SHARD({ index: 4 })]);
    expect((screen.getByLabelText(/Shard number for entry 1/) as HTMLInputElement).value).toBe('4');
  });

  it('refuses a bare hex share, which cannot be verified', async () => {
    openRecover();
    pasteShards(['aabbcc', 'ddeeff', '112233']);
    fireEvent.click(screen.getByText('Reconstruct Key'));

    await waitFor(() => {
      expect(screen.getByText(/Paste the whole shard/)).toBeInTheDocument();
    });
    expect(mockReconstructSecretKey).not.toHaveBeenCalled();
    expect(screen.queryByText(/reconstructed successfully/)).not.toBeInTheDocument();
  });

  it('refuses to reconstruct below the threshold instead of returning a wrong key', async () => {
    openRecover();
    // Two shards of a 3-of-5 set. Shamir would happily interpolate them into a
    // plausible 32-byte key that is not the secret, and say nothing.
    pasteShards([SHARD({ index: 1 }), SHARD({ index: 2 })]);
    fireEvent.click(screen.getByText('Reconstruct Key'));

    await waitFor(() => {
      expect(screen.getByText(/2 of the 3 shards needed/)).toBeInTheDocument();
    });
    expect(mockReconstructSecretKey).not.toHaveBeenCalled();
  });

  it('rejects the same shard entered twice', async () => {
    openRecover();
    pasteShards([SHARD({ index: 2 }), SHARD({ index: 2 }), SHARD({ index: 2 })]);
    fireEvent.click(screen.getByText('Reconstruct Key'));

    await waitFor(() => {
      expect(screen.getByText(/same shard was entered twice/)).toBeInTheDocument();
    });
    expect(mockReconstructSecretKey).not.toHaveBeenCalled();
  });

  it('rejects shards from two different identities', async () => {
    openRecover();
    pasteShards([
      SHARD({ index: 1 }),
      SHARD({ index: 2 }),
      SHARD({ index: 3, commitment: 'beefbeef' }),
    ]);
    fireEvent.click(screen.getByText('Reconstruct Key'));

    await waitFor(() => {
      expect(screen.getByText(/different identities/)).toBeInTheDocument();
    });
    expect(mockReconstructSecretKey).not.toHaveBeenCalled();
  });

  it('does not report success when the key fails to reproduce the commitment', async () => {
    mockReconstructSecretKey.mockReturnValue('00'.repeat(32));
    mockComputeCommitment.mockResolvedValue('not-the-commitment');

    openRecover();
    pasteShards([SHARD({ index: 1 }), SHARD({ index: 2 }), SHARD({ index: 3 })]);
    fireEvent.click(screen.getByText('Reconstruct Key'));

    await waitFor(() => {
      expect(screen.getByText(/did not reconstruct your identity/)).toBeInTheDocument();
    });
    expect(screen.queryByText(/reconstructed successfully/)).not.toBeInTheDocument();
  });

  it('accepts a reconstruction that reproduces the commitment', async () => {
    mockReconstructSecretKey.mockReturnValue('11'.repeat(32));
    mockComputeCommitment.mockResolvedValue('c0ffee');

    openRecover();
    pasteShards([SHARD({ index: 1 }), SHARD({ index: 2 }), SHARD({ index: 3 })]);
    fireEvent.click(screen.getByText('Reconstruct Key'));

    await waitFor(() => {
      expect(screen.getByText(/reconstructed successfully/)).toBeInTheDocument();
    });
    // Verified against the parameters the shards carry, not freshly minted ones.
    expect(mockComputeCommitment).toHaveBeenCalledWith(BigInt('0x' + '11'.repeat(32)), 1, 482913);
  });

  it('restores the original identity rather than deriving a new one', async () => {
    mockReconstructSecretKey.mockReturnValue('11'.repeat(32));
    mockComputeCommitment.mockResolvedValue('c0ffee');

    openRecover();
    pasteShards([SHARD({ index: 1 }), SHARD({ index: 2 }), SHARD({ index: 3 })]);
    fireEvent.click(screen.getByText('Reconstruct Key'));
    await waitFor(() => screen.getByText(/reconstructed successfully/));

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Import Recovered Key' }));
    });

    await waitFor(() => expect(mockImportBackup).toHaveBeenCalled());
    const backup = JSON.parse(mockImportBackup.mock.calls[0][0] as unknown as string);
    // It used to call generateNodeId() for a RANDOM node id and hardcode
    // roleCode 1, producing a different commitment — a new identity, not the
    // recovered one.
    expect(backup.nodeId).toBe(482913);
    expect(backup.roleCode).toBe(1);
    expect(backup.commitmentHex).toBe('c0ffee');
    expect(backup.sk).toBe('11'.repeat(32));
  });
});
