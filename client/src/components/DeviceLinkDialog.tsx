/**
 * Device linking dialog — transfers an identity between devices.
 * to another device, or accepts a scanned payload to import.
 *
 * Two modes:
 * - "share": Shows QR + pairing code for the target device to scan
 * - "receive": Accepts pasted QR data + pairing code to import identity
 */

import { useState, useCallback } from 'react';
import { useIdentityStore } from '@/stores/identity';
import {
  generatePairingCode,
  encryptIdentity,
  decryptIdentity,
  encodePayload,
  decodePayload,
  generateQrSvg,
} from '@/lib/device-link';
import * as db from '@/lib/db';
import { Modal } from '@/components/Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';

interface Props {
  onClose: () => void;
}

type Mode = 'choose' | 'share' | 'receive';

export function DeviceLinkDialog({ onClose }: Props) {
  const titleId = useDialogTitleId();
  const identity = useIdentityStore((s) => s.identity);
  const loadIdentities = useIdentityStore((s) => s.loadIdentities);

  const [mode, setMode] = useState<Mode>('choose');
  const [pairingCode, setPairingCode] = useState('');
  const [qrSvg, setQrSvg] = useState('');
  /**
   * The encoded payload, shown as selectable text.
   *
   * It used to be generated and then rendered ONLY as the QR-like graphic —
   * which `generateQrSvg` documents as "NOT a standards-compliant QR code and
   * cannot be scanned by QR reader apps". So the share screen offered a code
   * that could not be scanned and did not expose the payload that could be
   * pasted: the flow it describes was impossible to complete.
   */
  const [transferCode, setTransferCode] = useState('');
  const [copied, setCopied] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);

  // Receive mode state
  const [inputPayload, setInputPayload] = useState('');
  const [inputCode, setInputCode] = useState('');
  const [importing, setImporting] = useState(false);

  const handleShare = useCallback(async () => {
    if (!identity) return;
    setGenerating(true);
    setError(null);
    try {
      const code = generatePairingCode();
      const payload = await encryptIdentity(identity, code);
      const encoded = encodePayload(payload);
      const svg = generateQrSvg(encoded);
      setPairingCode(code);
      setQrSvg(svg);
      setTransferCode(encoded);
      setCopied(false);
      setMode('share');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to generate link');
    } finally {
      setGenerating(false);
    }
  }, [identity]);

  const handleImport = useCallback(async () => {
    setImporting(true);
    setError(null);
    try {
      const payload = decodePayload(inputPayload.trim());
      const imported = await decryptIdentity(payload, inputCode);

      // Generate a new local ID to avoid collisions
      imported.id = crypto.randomUUID();
      await db.saveIdentity(imported);
      await loadIdentities();
      setSuccess(true);
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message.includes('decrypt')
            ? 'Wrong pairing code or corrupted data'
            : e.message
          : 'Import failed',
      );
    } finally {
      setImporting(false);
    }
  }, [inputPayload, inputCode, loadIdentities]);

  return (
    <Modal onClose={onClose} className="device-link-dialog" titleId={titleId} focusKey={mode}>
      <h2 id={titleId}>Device Linking</h2>

      {mode === 'choose' && (
        <div className="device-link-choose">
          <p className="device-link-description">
            Transfer your identity to another device securely. No seed phrases, no manual exports.
          </p>
          <button
            className="device-link-option"
            onClick={handleShare}
            disabled={!identity || generating}
          >
            <span className="device-link-option-icon">&#x1F4F1;</span>
            <span className="device-link-option-text">
              <strong>Link a New Device</strong>
              <span>Show a transfer code to copy to your other device</span>
            </span>
          </button>
          <button
            className="device-link-option"
            onClick={() => setMode('receive')}
          >
            <span className="device-link-option-icon">&#x1F4F7;</span>
            <span className="device-link-option-text">
              <strong>Receive from Another Device</strong>
              <span>Paste a transfer code from your other device</span>
            </span>
          </button>
          <div className="dialog-actions">
            <button onClick={onClose}>Cancel</button>
          </div>
        </div>
      )}

      {mode === 'share' && (
        <div className="device-link-share">
          <p className="device-link-description">
            Copy the transfer code below into the "Receive" screen on your other
            device, then enter the pairing code to complete the transfer.
          </p>
          <div
            className="qr-container"
            aria-hidden="true"
            dangerouslySetInnerHTML={{ __html: qrSvg }}
          />
          <label className="transfer-code-label" htmlFor="transfer-code">
            Transfer code
          </label>
          <textarea
            id="transfer-code"
            className="transfer-code-value"
            value={transferCode}
            readOnly
            rows={3}
            onFocus={(e) => e.currentTarget.select()}
          />
          <button
            type="button"
            className="secondary-btn"
            onClick={() => {
              navigator.clipboard
                ?.writeText(transferCode)
                .then(() => setCopied(true))
                // Clipboard access can be denied; the field above is
                // selectable so the user still has a way through.
                .catch(() => setCopied(false));
            }}
          >
            {copied ? 'Copied!' : 'Copy transfer code'}
          </button>
          <div className="pairing-code-display">
            <span className="pairing-code-label">Pairing Code</span>
            <span className="pairing-code-value">{pairingCode}</span>
          </div>
          <p className="pairing-code-hint">
            Enter this code on your other device to complete the transfer.
          </p>
          <div className="dialog-actions">
            <button onClick={() => setMode('choose')}>Back</button>
            <button onClick={onClose}>Done</button>
          </div>
        </div>
      )}

      {mode === 'receive' && !success && (
        <div className="device-link-receive">
          <p className="device-link-description">
            Paste the QR data from your other device and enter the pairing code.
          </p>
          <label>
            QR Data
            <textarea
              value={inputPayload}
              onChange={(e) => setInputPayload(e.target.value)}
              placeholder='Paste the encoded data here...'
              rows={4}
              disabled={importing}
            />
          </label>
          <label>
            Pairing Code
            <input
              type="text"
              value={inputCode}
              onChange={(e) => setInputCode(e.target.value)}
              placeholder="000000"
              maxLength={6}
              pattern="[0-9]*"
              inputMode="numeric"
              disabled={importing}
            />
          </label>
          {error && <div className="error-message">{error}</div>}
          <div className="dialog-actions">
            <button onClick={() => setMode('choose')} disabled={importing}>
              Back
            </button>
            <button
              className="primary-btn"
              onClick={handleImport}
              disabled={importing || !inputPayload || inputCode.length !== 6}
            >
              {importing ? 'Decrypting...' : 'Import Identity'}
            </button>
          </div>
        </div>
      )}

      {mode === 'receive' && success && (
        <div className="device-link-success">
          <div className="success-message">Identity imported successfully!</div>
          <p>Your identity has been securely transferred to this device.</p>
          <div className="dialog-actions">
            <button className="primary-btn" onClick={onClose}>
              Continue
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}
