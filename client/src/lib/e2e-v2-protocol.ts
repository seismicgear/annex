/** Wire contract shared with annex-server/services/e2e_service.rs. */
import type { DeviceCertificate } from './device-certificate';
import type { ManifestProjection, WrapAuthorizationReceipt } from './e2e-manifest';
export interface ChannelKeyState {
  version: 2;
  server_identity: string;
  authority_identity: string;
  authority_server: string;
  revision: number;
  current_epoch: number;
  required_epoch: number;
  current_key_id: string | null;
  history_policy: 'all_members';
  e2e_enabled: boolean;
  has_key: boolean;
  rotation_required: boolean;
  legacy_history_available: boolean;
  max_epoch: number;
}

export interface DevicePublicKey {
  device_id: string;
  x25519_pub_hex: string;
  /** Null is preserved for legacy history, never eligible for a new key distribution. */
  device_certificate: DeviceCertificate | null;
}

export interface MemberDeviceKey extends DevicePublicKey {
  pseudonym_id: string;
  certificate_sha256: string;
  has_current_wrap: boolean;
}

export interface LegacyCursor { epoch: number; key_id: string }
export interface MissingEpochDevices extends LegacyCursor { authority_namespace: string; missing_devices: MemberDeviceKey[] }

export interface ChannelKeyDirectory extends ChannelKeyState {
  manifest: ManifestProjection;
  member_keys: MemberDeviceKey[];
  missing_current_devices: MemberDeviceKey[];
  historical_epochs: MissingEpochDevices[];
  next_history_after_epoch: number | null;
  legacy_candidates: MissingEpochDevices[];
  next_legacy_cursor: LegacyCursor | null;
}

export interface DeviceKeyWrap extends LegacyCursor {
  authority_namespace: string;
  authorization: WrapAuthorizationReceipt | null;
  recipient_device_id: string;
  sender_pseudonym_id: string;
  sender_device_id: string;
  wrapped_key_b64: string;
}

export interface LegacyKeyWrap {
  authority_namespace: string;
  epoch: number;
  key_epoch: number;
  sender_pseudonym_id: string;
  wrapped_key_b64: string;
}

export interface OwnChannelWraps {
  state: ChannelKeyState;
  wraps: DeviceKeyWrap[];
  next_after_epoch: number | null;
  legacy_device_wraps: DeviceKeyWrap[];
  next_legacy_cursor: LegacyCursor | null;
  legacy_wraps: LegacyKeyWrap[];
  next_legacy_wrap_after_epoch: number | null;
}

export interface ChannelWrapUpload {
  recipient_pseudonym_id: string;
  recipient_device_id: string;
  wrapped_key_b64: string;
}

export interface ChannelWrapCommit {
  version: 2;
  operation: 'commit_epoch' | 'admit_devices' | 'admit_history_devices' | 'admit_legacy_devices';
  expected_revision: number;
  epoch: number;
  key_id: string;
  sender_device_id: string;
  wraps: ChannelWrapUpload[];
}

export interface KeyPage {
  limit?: number;
  history_after_epoch?: number;
  after_epoch?: number;
  legacy_after_epoch?: number;
  legacy_after_key_id?: string;
  legacy_wrap_after_epoch?: number;
}

export interface AffectedChannelState extends ChannelKeyState { channel_id: string }

/** Bound to one captured server/identity context, including every post-await. */
export interface E2eApiV2 {
  assertCurrent(): void;
  getState(channelId: string): Promise<ChannelKeyState>;
  publishDevice(publicKeyHex: string): Promise<{ device_id: string; created: boolean; affected_channels: AffectedChannelState[] }>;
  getDevices(pseudonym: string): Promise<{ pseudonym_id: string; devices: DevicePublicKey[] }>;
  revokeDevice(deviceId: string): Promise<{ device_id: string; revoked: true; affected_channels: AffectedChannelState[] }>;
  getDirectory(channelId: string, page?: KeyPage): Promise<ChannelKeyDirectory>;
  getWraps(channelId: string, deviceId: string, page?: KeyPage): Promise<OwnChannelWraps>;
  postWraps(channelId: string, request: ChannelWrapCommit): Promise<{ status: 'ok'; inserted: number; state: ChannelKeyState }>;
}

export interface LocalEncryptionDevice {
  deviceId: string;
  publicKey: Uint8Array;
  secret: Uint8Array;
}

export interface RetainedChannelKey {
  authorityServer: string;
  epoch: number;
  keyId: string;
  cek: Uint8Array;
  purpose: 'current' | 'legacy';
  /** Only an authenticated legacy message proves a raw historical candidate. */
  verifiedLegacy?: boolean;
}

/** Every durable key namespace includes stable server identity and pseudonym. */
export interface E2eKeyringV2 {
  getOrCreateDevice(serverIdentity: string, pseudonym: string): Promise<LocalEncryptionDevice>;
  replaceDevice(serverIdentity: string, pseudonym: string, expectedDeviceId: string): Promise<LocalEncryptionDevice>;
  getLegacyDeviceSecrets(serverIdentity: string, pseudonym: string): Promise<Uint8Array[]>;
  getKeys(serverIdentity: string, pseudonym: string, channelId: string): Promise<RetainedChannelKey[]>;
  putKey(serverIdentity: string, pseudonym: string, channelId: string, key: RetainedChannelKey): Promise<void>;
  /** Reads the old unscoped store without rewriting or deleting its bytes. */
  getUnscopedLegacyKey(pseudonym: string, channelId: string): Promise<{ epoch: number; cek: Uint8Array } | null>;
}
