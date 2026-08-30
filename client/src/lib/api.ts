/**
 * HTTP API client for the Annex server.
 *
 * Compatibility re-export. The implementation now lives in `@/api/*`,
 * split by domain (core, identity, channels, messages, voice, federation,
 * rtx, admin, uploads, usernames). New code should import the specific
 * domain module directly; this barrel keeps existing call sites working.
 */

export {
  ApiError,
  authHeaders,
  fetchWithTimeout,
  getApiBaseUrl,
  getSessionToken,
  getZkProofPayload,
  isTokenExpired,
  refreshSessionToken,
  request,
  requestRemote,
  resolveUrl,
  setApiBaseUrl,
  setSessionToken,
  setZkProofPayload,
  startTokenRefresh,
  stopTokenRefresh,
} from '@/api/core';

export {
  createInvite,
  getCurrentRoot,
  getIdentityInfo,
  redeemInvite,
  register,
  verifyMembership,
} from '@/api/identity';

export {
  createChannel,
  deleteChannel,
  getChannel,
  joinChannel,
  leaveChannel,
  listChannels,
} from '@/api/channels';

export type { MessageSearchResult } from '@/api/messages';
export {
  getMessageEdits,
  getMessages,
  searchMessages,
} from '@/api/messages';

export type {
  IceServerConfig,
  JoinVoiceResponse,
  VoiceConfigStatus,
} from '@/api/voice';
export {
  getVoiceConfigStatus,
  getVoiceStatus,
  joinVoice,
  leaveVoice,
} from '@/api/voice';

export {
  getFederationPeers,
  getRemoteFederationPeers,
  getRemoteServerSummary,
  getServerSummary,
} from '@/api/federation';

export {
  getPublicAgents,
  getPublicEvents,
} from '@/api/rtx';

export type { MemberInfo } from '@/api/admin';
export {
  getPolicy,
  getServer,
  listMembers,
  renameServer,
  setPublicUrl,
  setWebrtcPublicUrl,
  updateMemberCapabilities,
  updatePolicy,
} from '@/api/admin';

export type { UploadResponse } from '@/api/uploads';
export {
  getServerImage,
  uploadChatFile,
  uploadChatImage,
  uploadServerImage,
} from '@/api/uploads';

export {
  deleteUsername,
  getVisibleUsernames,
  grantUsername,
  listUsernameGrants,
  revokeUsernameGrant,
  setUsername,
} from '@/api/usernames';

export type { KeyWrapRecord, KeyWrapUpload, MemberKey } from '@/api/e2e';
export {
  getChannelE2e,
  getChannelKeyStatus,
  getChannelKeyWraps,
  getChannelMemberKeys,
  getMemberKey,
  postChannelKeyWraps,
  publishMyKey,
  setChannelE2e,
} from '@/api/e2e';
