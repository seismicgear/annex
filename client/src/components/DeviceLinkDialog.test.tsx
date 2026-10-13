import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { DeviceLinkDialog } from './DeviceLinkDialog';
import { decodePayload, decryptIdentity, encodePayload, encryptIdentity } from '@/lib/device-link';
import type { StoredIdentity } from '@/types';

let identity: StoredIdentity;
vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (value: unknown) => unknown) => selector({ identity, loadIdentities: vi.fn() }),
}));

beforeEach(() => {
  identity = {
    id: 'local-test', sk: 'a'.repeat(64), roleCode: 1, nodeId: 7,
    commitmentHex: 'b'.repeat(64), serverSlug: 'test', leafIndex: 0,
    pseudonymId: null, sessionToken: null, createdAt: '2026-01-01T00:00:00Z',
  };
});

describe('device linking share controls', () => {
  it('shows a visible retryable error when browser decryption fails without a message', async () => {
    const user = userEvent.setup();
    const encoded = encodePayload(await encryptIdentity(identity, '123456'));
    const decrypt = vi.spyOn(crypto.subtle, 'decrypt').mockRejectedValueOnce(new DOMException('', 'OperationError'));
    try {
      render(<DeviceLinkDialog onClose={vi.fn()} />);
      await user.click(screen.getByRole('button', { name: /Receive from Another Device/ }));
      await user.click(screen.getByLabelText('Transfer code'));
      await user.paste(encoded);
      await user.type(screen.getByLabelText('Pairing Code'), '654321');
      await user.click(screen.getByRole('button', { name: 'Import Identity' }));
      expect(await screen.findByRole('alert')).toHaveTextContent('Wrong pairing code or corrupted data');
      expect(screen.getByRole('button', { name: 'Import Identity' })).toBeEnabled();
      expect(screen.queryByText('Identity imported successfully!')).toBeNull();
    } finally {
      decrypt.mockRestore();
    }
  });

  it('shares a real encrypted payload and a scannable QR graphic', async () => {
    const user = userEvent.setup();
    render(<DeviceLinkDialog onClose={vi.fn()} />);
    await user.click(screen.getByRole('button', { name: /Link a New Device/ }));
    const transfer = await screen.findByLabelText('Transfer code');
    const encoded = (transfer as HTMLTextAreaElement).value;
    const code = document.querySelector('.pairing-code-value')!.textContent!;
    expect(await decryptIdentity(decodePayload(encoded), code)).toMatchObject({
      sk: identity.sk, commitmentHex: identity.commitmentHex, sessionToken: null, leafIndex: null,
    });
    expect(screen.getByRole('img', { name: 'Encrypted identity transfer QR code' }).querySelector('svg')).not.toBeNull();
  });

  it('does not put a registered identity proof cache or session in the QR transfer', async () => {
    identity.zkProofPayload = 'x'.repeat(10_000);
    identity.sessionToken = 'source-session-token';
    identity.pseudonymId = 'c'.repeat(64);
    const user = userEvent.setup();
    render(<DeviceLinkDialog onClose={vi.fn()} />);
    await user.click(screen.getByRole('button', { name: /Link a New Device/ }));
    const transfer = await screen.findByLabelText('Transfer code') as HTMLTextAreaElement;
    const code = document.querySelector('.pairing-code-value')!.textContent!;
    const imported = await decryptIdentity(decodePayload(transfer.value), code);
    expect(imported).toMatchObject({ sk: identity.sk, commitmentHex: identity.commitmentHex, pseudonymId: identity.pseudonymId, sessionToken: null });
    expect(imported.zkProofPayload).toBeUndefined();
    expect(screen.getByRole('img', { name: 'Encrypted identity transfer QR code' })).toBeVisible();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('reports a clipboard refusal beside the selectable transfer text', async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValueOnce(new DOMException('Denied', 'NotAllowedError'));
    render(<DeviceLinkDialog onClose={vi.fn()} />);
    await user.click(screen.getByRole('button', { name: /Link a New Device/ }));
    await screen.findByLabelText('Transfer code');
    await user.click(screen.getByRole('button', { name: 'Copy transfer code' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/copy it manually/i);
    expect(screen.getByLabelText('Transfer code')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Copied!' })).toBeNull();
  });
});
