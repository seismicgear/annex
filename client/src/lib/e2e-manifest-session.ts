/** Verifies paged lineage before any recipient directory or plaintext policy is usable. */
import { E2eError } from './e2e';
import type { DeviceCertificate } from './device-certificate';
import { applyManifestEvent, checkManifestPin, emptyManifest, manifestProjection, signManifestEvent,
  type JoinPolicy, type ManifestOperation, type ManifestPage, type ManifestPin, type ManifestProjection,
  type SignedManifestEvent, type VerifiedManifest } from './e2e-manifest';

export interface ManifestHistory { final: VerifiedManifest; history: Map<number, VerifiedManifest>; trusted: boolean }
export interface ManifestTrustPrompt { authorityIdentity: string; channelId: string; genesisHash: string }
export class E2eTrustRequiredError extends E2eError {
  constructor(readonly prompt: ManifestTrustPrompt) { super('Verify this channel’s encryption before sending or receiving new keys.'); }
}
export class E2eRecoveryRequiredError extends E2eError {
  constructor() { super('This identity has a new encryption device. Recover channel access to replace its previously admitted devices.'); }
}
export interface ManifestTransport {
  assertCurrent(): void;
  read(channelId: string, after: number): Promise<ManifestPage>;
  write(channelId: string, event: SignedManifestEvent): Promise<{ event: SignedManifestEvent; manifest: ManifestProjection }>;
  signer(): Promise<{ certificate: DeviceCertificate; secret: Uint8Array }>;
  pin(authorityIdentity: string, channelId: string): Promise<ManifestPin | null>;
  retain(history: ManifestHistory, firstUse: boolean): Promise<void>;
}

export class ChannelManifestSession {
  constructor(private readonly pseudonym: string, private readonly transport: ManifestTransport) {}

  async inspect(channelId: string, authorityIdentity: string): Promise<ManifestHistory> {
    this.transport.assertCurrent();
    let state = emptyManifest(authorityIdentity, channelId);
    const history = new Map<number, VerifiedManifest>([[0, state]]);
    let after = 0;
    let advertised: { head: number; hash: string; genesis: string | null } | null = null;
    for (let pageNumber = 0; pageNumber < 1000; pageNumber++) {
      const page = await this.transport.read(channelId, after);
      this.transport.assertCurrent();
      if (page.authority_identity !== authorityIdentity || page.channel_id !== channelId) throw new E2eError('Manifest authority or channel changed');
      if (advertised && (page.head_seq !== advertised.head || page.head_hash !== advertised.hash || page.genesis_hash !== advertised.genesis)) {
        throw Object.assign(new E2eError('Channel membership changed while its history was loading. Retry.'), { status: 409 });
      }
      advertised ??= { head: page.head_seq, hash: page.head_hash, genesis: page.genesis_hash };
      for (const event of page.events) {
        state = await applyManifestEvent(state, event);
        this.transport.assertCurrent();
        history.set(event.seq, state);
      }
      if (page.next_after_seq === null) {
        if ((state.head?.seq ?? 0) !== advertised.head || (state.head?.hash ?? '') !== advertised.hash
          || (state.head?.genesisHash ?? null) !== advertised.genesis) throw new E2eError('Manifest history is incomplete');
        const pin = await this.transport.pin(authorityIdentity, channelId);
        this.transport.assertCurrent();
        checkManifestPin(pin, history, state);
        const result = { final: state, history, trusted: pin !== null };
        if (pin) await this.transport.retain(result, false);
        this.transport.assertCurrent();
        return result;
      }
      if (!page.events.length || page.next_after_seq <= after || page.next_after_seq !== state.head?.seq) throw new E2eError('Invalid manifest page cursor');
      after = page.next_after_seq;
    }
    throw new E2eError('Channel membership history exceeds the synchronization limit');
  }

  requireTrusted(value: ManifestHistory): void {
    if (!value.final.head) throw new E2eError('A channel moderator must initialize verified encryption before new keys can be shared.');
    if (!value.trusted) throw new E2eTrustRequiredError({ authorityIdentity: value.final.authorityIdentity,
      channelId: value.final.channelId, genesisHash: value.final.head.genesisHash });
  }

  async trust(channelId: string, authority: string, expectedGenesis: string): Promise<ManifestHistory> {
    const value = await this.inspect(channelId, authority);
    if (value.final.head?.genesisHash !== expectedGenesis) throw new E2eError('The channel changed while its fingerprint was being verified.');
    await this.transport.retain(value, true);
    this.transport.assertCurrent();
    return { ...value, trusted: true };
  }

  async apply(value: ManifestHistory, operation: ManifestOperation, trustOwnGenesis = false): Promise<ManifestHistory> {
    if (operation.kind !== 'genesis') this.requireTrusted(value);
    else if (!trustOwnGenesis || value.final.head) throw new E2eError('Only an explicitly created channel genesis can establish first trust.');
    const signing = await this.transport.signer();
    this.transport.assertCurrent();
    let event: SignedManifestEvent;
    try {
      event = signManifestEvent({ authority_identity: value.final.authorityIdentity, channel_id: value.final.channelId,
        seq: (value.final.head?.seq ?? 0) + 1, previous_hash: value.final.head?.hash ?? '', operation }, signing.certificate, signing.secret);
    } finally { signing.secret.fill(0); }
    const proposed = await applyManifestEvent(value.final, event);
    this.transport.assertCurrent();
    const response = await this.transport.write(value.final.channelId, event);
    this.transport.assertCurrent();
    if (response.event.event_hash !== event.event_hash || JSON.stringify(response.manifest) !== JSON.stringify(manifestProjection(proposed))) {
      throw new E2eError('The authority did not confirm the signed membership change');
    }
    const history = new Map(value.history).set(event.seq, proposed);
    const result = { final: proposed, history, trusted: true };
    await this.transport.retain(result, trustOwnGenesis);
    this.transport.assertCurrent();
    return result;
  }

  async ensureAdmitted(value: ManifestHistory): Promise<ManifestHistory> {
    this.requireTrusted(value);
    const signing = await this.transport.signer();
    signing.secret.fill(0);
    this.transport.assertCurrent();
    const { certificate } = signing;
    const member = value.final.members.get(this.pseudonym);
    if (member?.devices.has(certificate.device_id)) return value;
    if (member) throw new E2eRecoveryRequiredError();
    if (value.final.joinPolicy !== 'eligible_self_join') throw new E2eError('A channel moderator must admit this identity before it can receive encryption keys.');
    return this.apply(value, { kind: 'join', certificate });
  }

  async enable(value: ManifestHistory, joinPolicy: JoinPolicy): Promise<ManifestHistory> {
    if (value.final.head) return this.apply(value, { kind: 'set_encryption', enabled: true });
    const signer = await this.transport.signer();
    signer.secret.fill(0);
    this.transport.assertCurrent();
    return this.apply(value, { kind: 'genesis', certificate: signer.certificate, join_policy: joinPolicy, history_policy: 'all_members' }, true);
  }

  async recover(value: ManifestHistory): Promise<ManifestHistory> {
    this.requireTrusted(value);
    const signer = await this.transport.signer();
    signer.secret.fill(0);
    this.transport.assertCurrent();
    return this.apply(value, { kind: 'recover_device', certificate: signer.certificate });
  }
}
