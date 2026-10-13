/** Encryption status, key retry and moderator controls for the selected channel. */
import { useState, useSyncExternalStore } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { Modal } from './Modal';
import { getChannelTrustPrompt } from '@/lib/message-crypto';
import { getE2eControls } from '@/lib/e2e-store';
import { EncryptionDevices } from './EncryptionDevices';
import { EncryptionAccess } from './EncryptionAccess';
import { getVerifiedChannel, subscribeVerifiedChannels } from '@/lib/verified-channel';

export function ChannelEncryptionBar() {
  const channelId = useChannelsStore(state => state.activeChannelId);
  return channelId ? <ChannelEncryptionControls key={channelId} /> : null;
}

function ChannelEncryptionControls() {
  const channelId = useChannelsStore(state => state.activeChannelId)!;
  const e2e = useChannelsStore(state => state.activeChannelE2e);
  const keyState = useChannelsStore(state => state.activeChannelKeyState);
  const keyError = useChannelsStore(state => state.activeChannelKeyError);
  const enable = useChannelsStore(state => state.enableChannelE2e);
  const disable = useChannelsStore(state => state.disableChannelE2e);
  const retry = useChannelsStore(state => state.retryChannelKey);
  const session = useChannelsStore(state => state.wsSessionState);
  const pseudonym = useIdentityStore(state => state.identity?.pseudonymId);
  const serverModerator = useIdentityStore(state => state.permissions?.capabilities.can_moderate ?? false);
  const verified = useSyncExternalStore(subscribeVerifiedChannels, () => pseudonym ? getVerifiedChannel(pseudonym, channelId) : null);
  const moderator = !!verified?.trusted && verified.members.some(member => member.pseudonym === pseudonym && member.role === 'moderator');
  const canEnable = moderator || (!!verified && !verified.genesisHash && serverModerator);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDisable, setConfirmDisable] = useState(false);
  const [verifyOpen, setVerifyOpen] = useState(false);
  const [recoverOpen, setRecoverOpen] = useState(false);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [accessOpen, setAccessOpen] = useState(false);
  const trust = keyState === 'unverified' ? getChannelTrustPrompt(channelId) : null;
  const blocked = busy || session === 'taken_over' || session === 'revoked' || !pseudonym;

  const perform = async (action: (pseudonym: string) => Promise<void>) => {
    if (blocked || !pseudonym) return;
    setBusy(true);
    setError(null);
    try { await action(pseudonym); }
    catch (reason) { setError(reason instanceof Error && reason.message ? reason.message : 'The encryption action could not be completed. Try again.'); }
    finally { setBusy(false); }
  };

  if (!e2e && !canEnable && !verified?.genesisHash && keyState === 'ready') return null;
  return <>
    <div className={`channel-encryption-bar ${e2e ? `encrypted key-${keyState}` : ''}`} role="region" aria-label="Channel encryption">
      {keyState === 'unverified' ? <>
        <span role="status">Verify this channel’s encryption before sending or receiving new keys.</span>
        <button type="button" disabled={blocked || !trust} onClick={() => { setError(null); setVerifyOpen(true); }}>Verify encryption</button>
      </> : keyState === 'recovery' ? <>
        <span role="status">This identity has a new encryption device. Channel access needs recovery.</span>
        <button type="button" disabled={blocked} onClick={() => { setError(null); setRecoverOpen(true); }}>Recover encryption access</button>
      </> : e2e ? <>
        {keyState === 'pending' ? <span role="status">Encrypted. Waiting for this device’s channel keys from an authorized member.</span>
          : keyState === 'failed' ? <span role="alert">The channel key could not be loaded{keyError ? `: ${keyError}` : ''}. Sending remains blocked.</span>
            : <span role="status">End-to-end encrypted — only authorized members can read these messages.</span>}
        {keyState !== 'ready' && <button type="button" disabled={blocked} onClick={() => void perform(retry)}>
          {busy ? 'Checking…' : keyState === 'pending' ? 'Check again' : 'Retry'}
        </button>}
        {moderator && <button type="button" disabled={blocked} onClick={() => { setError(null); setConfirmDisable(true); }}>Disable encryption</button>}
        {keyState === 'ready' && <button type="button" disabled={blocked} onClick={() => setDevicesOpen(true)}>Encryption devices</button>}
      </> : keyState !== 'ready' ? <>
        {keyState === 'pending' ? <span role="status">Checking channel encryption before sending…</span>
          : <span role="alert">Encryption status could not be verified{keyError ? `: ${keyError}` : ''}. Sending remains blocked.</span>}
        <button type="button" disabled={blocked} onClick={() => void perform(retry)}>Retry</button>
      </> : <>
        <span>Messages in this channel are stored on the server. Enable end-to-end encryption so only authorized members can read new messages.</span>
        {canEnable && <button type="button" disabled={blocked} onClick={() => void perform(enable)}>{busy ? 'Enabling…' : '🔒 Enable encryption'}</button>}
      </>}
      {verified?.trusted && verified.genesisHash && <button type="button" disabled={blocked} onClick={() => setAccessOpen(true)}>Encryption access</button>}
      {error && !confirmDisable && !verifyOpen && !recoverOpen && <span className="channel-encryption-error" role="alert">{error}</span>}
    </div>
    {devicesOpen && pseudonym && <EncryptionDevices channelId={channelId} pseudonym={pseudonym} onClose={() => setDevicesOpen(false)} />}
    {accessOpen && pseudonym && <EncryptionAccess channelId={channelId} pseudonym={pseudonym} onClose={() => setAccessOpen(false)} />}
    {verifyOpen && trust && <Modal label="Verify encryption" onClose={() => { if (!busy) setVerifyOpen(false); }}>
      <h2>Verify encryption</h2>
      <p>Confirm these fingerprints with a channel member you trust. This device will remember the channel’s original signed membership history and refuse a replacement or rollback.</p>
      <dl className="encryption-fingerprints">
        <dt>Authority</dt><dd><code>{trust.authorityIdentity}</code></dd>
        <dt>Channel fingerprint</dt><dd><code>{trust.genesisHash}</code></dd>
      </dl>
      {error && <p role="alert">{error}</p>}
      <div className="dialog-actions">
        <button type="button" disabled={busy} onClick={() => setVerifyOpen(false)}>Cancel</button>
        <button type="button" disabled={blocked} onClick={() => void perform(async id => {
          await getE2eControls(id).trustChannel(channelId, trust.authorityIdentity, trust.genesisHash);
          await retry(id); setVerifyOpen(false);
        })}>{busy ? 'Verifying…' : 'Trust this channel'}</button>
      </div>
    </Modal>}
    {recoverOpen && <Modal label="Recover encryption access" onClose={() => { if (!busy) setRecoverOpen(false); }}>
      <h2>Recover encryption access</h2>
      <p>If another device is available, reconnect there and approve this device under Encryption devices to keep both devices and share history keys.</p>
      <p>Recovery replaces this identity’s previously admitted devices and rotates future channel keys. Encrypted history stays stored, but reading it still requires a saved history key or another authorized member who can share it.</p>
      {error && <p role="alert">{error}</p>}
      <div className="dialog-actions">
        <button type="button" disabled={busy} onClick={() => setRecoverOpen(false)}>Cancel</button>
        <button type="button" disabled={blocked} onClick={() => void perform(async id => {
          await getE2eControls(id).recoverChannel(channelId); await retry(id); setRecoverOpen(false);
        })}>{busy ? 'Recovering…' : 'Recover on this device'}</button>
      </div>
    </Modal>}
    {confirmDisable && <Modal label="Disable channel encryption" onClose={() => { if (!busy) setConfirmDisable(false); }}>
      <h2>Disable channel encryption?</h2>
      <p>New messages will be stored on the server without end-to-end encryption. Existing encrypted messages and their history keys will be preserved.</p>
      {error && <p role="alert">{error}</p>}
      <div className="dialog-actions">
        <button type="button" disabled={busy} onClick={() => setConfirmDisable(false)}>Keep encryption</button>
        <button type="button" disabled={blocked} onClick={() => void perform(async id => { await disable(id); setConfirmDisable(false); })}>
          {busy ? 'Disabling…' : 'Disable encryption'}
        </button>
      </div>
    </Modal>}
  </>;
}
