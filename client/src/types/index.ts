/** Participant types matching server-side enum. */
export type ParticipantType = 'HUMAN' | 'AI_AGENT' | 'COLLECTIVE' | 'BRIDGE' | 'SERVICE';

/** VRP alignment status from handshake. */
export type AlignmentStatus = 'Aligned' | 'Partial' | 'Conflict';

/** VRP transfer scope from negotiation. */
export type TransferScope = 'NoTransfer' | 'ReflectionSummariesOnly' | 'FullKnowledgeBundle';

/** Channel types matching server enum (PascalCase from Rust serde). */
export type ChannelType = 'Text' | 'Voice' | 'Hybrid' | 'Agent' | 'Broadcast';

/** Federation scope for channels (PascalCase from Rust serde). */
export type FederationScope = 'Local' | 'Federated';

/** Stored identity keys in IndexedDB. */
export interface StoredIdentity {
  /** Unique key for IndexedDB storage. */
  id: string;
  /** Secret key (BN254 scalar as hex string). */
  sk: string;
  /** Role code (1=Human, 2=AI_Agent, etc.). */
  roleCode: number;
  /** Node ID for identity commitment. */
  nodeId: number;
  /** Computed identity commitment hex. */
  commitmentHex: string;
  /** Pseudonym ID received after membership verification. */
  pseudonymId: string | null;
  /** The server slug this identity is registered on. */
  serverSlug: string;
  /** Merkle leaf index assigned during registration. */
  leafIndex: number | null;
  /** Timestamp of creation. */
  createdAt: string;
}

/** Registration response from POST /api/registry/register. */
export interface RegistrationResponse {
  identityId: number;
  leafIndex: number;
  rootHex: string;
  pathElements: string[];
  pathIndexBits: number[];
}

/** Membership verification response from POST /api/zk/verify-membership. */
export interface VerifyMembershipResponse {
  ok: boolean;
  pseudonymId: string;
}

/** Capability flags from server. */
export interface Capabilities {
  can_voice: boolean;
  can_moderate: boolean;
  can_invite: boolean;
  can_federate: boolean;
  can_bridge: boolean;
}

/** Identity info from GET /api/identity/:pseudonymId. */
export interface IdentityInfo {
  pseudonymId: string;
  participantType: ParticipantType;
  active: boolean;
  capabilities: Capabilities;
}

/** Channel metadata from API. */
export interface Channel {
  channel_id: string;
  name: string;
  channel_type: ChannelType;
  topic: string | null;
  federation_scope: FederationScope;
}

/** Message from API or WebSocket. */
export interface Message {
  message_id: string;
  channel_id: string;
  sender_pseudonym: string;
  content: string;
  reply_to_message_id: string | null;
  created_at: string;
}

/** WebSocket frame for sending messages. */
export interface WsSendFrame {
  type: 'message';
  channelId: string;
  content: string;
  replyTo: string | null;
}

/** WebSocket frame received from server. */
export interface WsReceiveFrame {
  type: 'message' | 'rtx_bundle' | 'transcription' | 'error';
  // Message fields (camelCase from WsMessagePayload)
  channelId?: string;
  messageId?: string;
  senderPseudonym?: string;
  content?: string;
  replyToMessageId?: string | null;
  createdAt?: string;
  // Transcription fields
  speakerPseudonym?: string;
  text?: string;
  // Error fields
  error?: string;
  message?: string;
}

/** Agent info from GET /api/public/agents or /api/agents/:id. */
export interface AgentInfo {
  pseudonym_id: string;
  alignment_status: AlignmentStatus;
  transfer_scope: TransferScope;
  reputation_score: number;
  capabilities: string[];
  active: boolean;
}

/** Federation peer info from GET /api/public/federation/peers. */
export interface FederationPeer {
  instance_id: number;
  base_url: string;
  label: string;
  alignment_status: AlignmentStatus;
  transfer_scope: TransferScope;
}

/** Server summary from GET /api/public/server/summary. */
export interface ServerSummary {
  slug: string;
  label: string;
  members_by_type: Record<string, number>;
  total_active_members: number;
  channel_count: number;
  federation_peer_count: number;
  active_agent_count: number;
}

/** Graph node for presence. */
export interface GraphNode {
  pseudonym_id: string;
  node_type: ParticipantType;
  active: boolean;
  last_seen_at: string | null;
}

/** Public event from event log. */
export interface PublicEvent {
  id: number;
  domain: string;
  event_type: string;
  entity_type: string;
  entity_id: string;
  seq: number;
  payload_json: string;
  occurred_at: string;
}

/** Rate limiting configuration (matches server RateLimitConfig). */
export interface RateLimitConfig {
  registration_limit: number;
  verification_limit: number;
  default_limit: number;
}

/** Server access mode. */
export type AccessMode = 'public' | 'invite_only' | 'password';

/** Server policy (matches server ServerPolicy). */
export interface ServerPolicy {
  agent_min_alignment_score: number;
  agent_required_capabilities: string[];
  federation_enabled: boolean;
  default_retention_days: number;
  voice_enabled: boolean;
  max_members: number;
  rate_limit: RateLimitConfig;
  principles: string[];
  prohibited_actions: string[];
  access_mode: AccessMode;
  access_password: string;
  images_enabled: boolean;
  videos_enabled: boolean;
  files_enabled: boolean;
  max_image_size_mb: number;
  max_video_size_mb: number;
  max_file_size_mb: number;
  usernames_enabled: boolean;
}

// ── Multi-Server Hub ──

/** A saved server connection in the user's local node hub. */
export interface SavedServer {
  /** Unique local ID. */
  id: string;
  /** Server's public base URL (e.g. "https://annex.example.com"). */
  baseUrl: string;
  /** Server slug identifier. */
  slug: string;
  /** Human-readable server label. */
  label: string;
  /** The StoredIdentity.id registered on this server. */
  identityId: string;
  /** The Persona.id mapped to this server (if set). */
  personaId: string | null;
  /** Accent color for this server context (hex). */
  accentColor: string;
  /** VRP topic binding for this server. */
  vrpTopic: string;
  /** Last successful connection timestamp. */
  lastConnectedAt: string;
  /** Cached server summary for instant display. */
  cachedSummary: ServerSummary | null;
}

// ── Device Linking ──

/** Payload encoded in QR code for device-to-device identity transfer. */
export interface DeviceLinkPayload {
  /** Protocol version. */
  v: 1;
  /** Encrypted identity bundle (base64). */
  data: string;
  /** Initialization vector for AES-GCM (base64). */
  iv: string;
  /** Salt used to derive AES key from pairing code (base64). */
  salt: string;
}

// ── Invite Routing ──

/** State carried by an invite link for instant channel routing. */
export interface InvitePayload {
  /** Target server host (e.g. "annex.example.com"). */
  server: string;
  /** Channel ID to join after identity is ready. */
  channelId: string;
  /** Server slug for registration. */
  serverSlug: string;
  /** Optional human-readable server label. */
  label?: string;
}

// ── Persona Management ──

/** A user-defined persona that maps to a server-scoped pseudonym. */
export interface Persona {
  /** Unique local ID. */
  id: string;
  /** User-chosen display name (e.g. "seismicgear", "Jane Doe"). */
  displayName: string;
  /** Optional avatar data URL. */
  avatarUrl: string | null;
  /** The identity ID this persona is bound to. */
  identityId: string;
  /** Server slug scope. */
  serverSlug: string;
  /** Optional bio / status text. */
  bio: string;
  /** Color accent for this persona (hex). */
  accentColor: string;
  /** Timestamp of creation. */
  createdAt: string;
}

// ── Link Preview ──

/** Extracted OpenGraph / meta data from a URL. */
export interface LinkPreviewData {
  /** The original URL. */
  url: string;
  /** Page title from og:title or <title>. */
  title: string | null;
  /** Description from og:description or meta description. */
  description: string | null;
  /** Image URL from og:image. */
  imageUrl: string | null;
  /** Site name from og:site_name. */
  siteName: string | null;
  /** Whether the fetch is still loading. */
  loading: boolean;
  /** Error message if fetch failed. */
  error: string | null;
}

// ── Social Recovery ──

/** A shard of a Shamir-split secret key, held by a trusted peer. */
export interface RecoveryShard {
  /** Shard index (1-based). */
  index: number;
  /** The shard data (hex-encoded). */
  data: string;
  /** Pseudonym ID of the peer holding this shard. */
  holderPseudonymId: string;
  /** User-friendly label for the holder. */
  holderLabel: string;
}

/** Configuration for a social recovery setup. */
export interface RecoveryConfig {
  /** Identity ID this recovery config protects. */
  identityId: string;
  /** Total number of shards distributed. */
  totalShards: number;
  /** Minimum shards needed to reconstruct. */
  threshold: number;
  /** Metadata about each shard holder. */
  shards: RecoveryShard[];
  /** When the recovery was set up. */
  createdAt: string;
}
