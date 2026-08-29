/**
 * Voice store — manages persistent voice/video call state.
 *
 * Call state lives here (not in component useState) so it survives
 * tab switches, channel switches, and view changes — matching the
 * Discord pattern where the call stays connected in the background.
 */

import { create } from 'zustand';
import * as api from '@/lib/api';

export interface JoinError {
  /** Human-readable display message. */
  display: string;
  /** Machine-readable error code (e.g. 'voice_not_configured'). */
  code: string | null;
  /** Server-provided setup guidance, if any. */
  setupHint: string | null;
}

function getJoinErrorMessage(error: unknown): JoinError {
  if (error instanceof api.ApiError) {
    const body = error.message?.trim();
    if (!body) return { display: `Failed to join voice (${error.status})`, code: null, setupHint: null };

    try {
      const parsed = JSON.parse(body) as { error?: string; message?: string; setup_hint?: string };
      const display = parsed.message ?? parsed.error ?? body;
      return {
        display,
        code: parsed.error ?? null,
        setupHint: parsed.setup_hint ?? null,
      };
    } catch {
      return { display: body, code: null, setupHint: null };
    }
  }

  if (error instanceof Error && error.message) {
    return { display: error.message, code: null, setupHint: null };
  }

  return { display: 'Failed to join voice', code: null, setupHint: null };
}

/** WebRTC room connection lifecycle state. */
export type ConnectionState = 'idle' | 'connecting' | 'connected' | 'failed';

export interface VoiceState {
  /** WebRTC access token for the current session. */
  voiceToken: string | null;
  /** WebRTC server URL. */
  webrtcUrl: string | null;
  /** ICE (STUN/TURN) servers for WebRTC NAT traversal. */
  iceServers: api.IceServerConfig[];
  /** Channel ID the call is connected to. */
  connectedChannelId: string | null;
  /** Per-channel join-in-progress status (keyed by channelId). */
  joiningByChannel: Record<string, boolean>;
  /** WebRTC room connection state. */
  connectionState: ConnectionState;
  /** Error string from connection failure. */
  connectionError: string | null;
  /** Per-channel call-active status (keyed by channelId). */
  callActiveByChannel: Record<string, boolean>;
  /** Pseudonyms in each channel's call, from the last `checkCallActive` poll. */
  participantsByChannel: Record<string, string[]>;
  /** Per-channel join error (keyed by channelId). */
  joinErrorByChannel: Record<string, JoinError | null>;
  /** Whether the user has self-deafened (output muted). */
  deafened: boolean;
  /** Whether the local microphone is muted (shared source of truth). */
  micMuted: boolean;

  /** Channel ID that last failed (persisted after room teardown so VoicePanel can show error). */
  lastFailedChannelId: string | null;
  /** Monotonic join request counter — only the latest join can commit state. */
  activeJoinRequestId: number;
  /** True while any join request is in flight (disables join buttons globally). */
  joiningAnyCall: boolean;
  /** User-visible error from the last mic toggle failure. */
  micToggleError: string | null;

  /** Audio settings persisted across sessions. */
  inputDeviceId: string | null;
  outputDeviceId: string | null;
  outputVolume: number;  // 0–100
  /** Camera device ID (persisted). */
  cameraDeviceId: string | null;

  /** True when voice was unavailable at startup (e.g. WebRTC failed to start). */
  voiceSessionDisabled: boolean;
  /** Reason voice is disabled for this session. */
  voiceSessionDisabledReason: string | null;
  /** Mark voice as disabled for this session. */
  setVoiceSessionDisabled: (disabled: boolean, reason?: string) => void;

  /** Join a voice call on the given channel. */
  joinCall: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Leave the current voice call. */
  leaveCall: (pseudonymId: string) => Promise<void>;
  /** Toggle self-deafen state. */
  toggleDeafen: () => void;
  /** Toggle microphone mute (shared source of truth). */
  toggleMicMuted: () => void;
  /** Set microphone muted state explicitly. */
  setMicMuted: (muted: boolean) => void;
  /** Update audio settings. */
  setInputDevice: (deviceId: string | null) => void;
  setOutputDevice: (deviceId: string | null) => void;
  setOutputVolume: (vol: number) => void;
  setCameraDevice: (deviceId: string | null) => void;
  /** Update the WebRTC room connection state. */
  setConnectionState: (state: ConnectionState, error?: string | null) => void;
  /** Shared async mic toggle — updates store only after WebRTC succeeds. */
  /** Check if a call is active on a channel (for polling). */
  checkCallActive: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Get call-active status for a specific channel. */
  isCallActive: (channelId: string) => boolean;
  /** Get join error for a specific channel. */
  getJoinError: (channelId: string) => JoinError | null;
  /** Check if a join is in progress for a specific channel. */
  isJoining: (channelId: string) => boolean;
  /** Clear cached call status for a channel (used on channel switch). */
  clearChannelCallState: (channelId: string) => void;
  /** Handle an unexpected disconnect — clears session state and records a user-visible error. */
  handleUnexpectedDisconnect: (errorMessage?: string) => void;
  /** Force-clear all voice session state (used by server switching). */
  forceReset: () => void;
  /** Dismiss the persisted failure state (lastFailedChannelId + connectionError). */
  dismissConnectionError: () => void;
  /** Clear the mic toggle error. */
  setMicToggleError: (message: string | null) => void;
  clearMicToggleError: () => void;
}

/** Load saved audio settings from localStorage. */
function loadAudioSettings() {
  try {
    const raw = localStorage.getItem('annex:audioSettings');
    if (raw) return JSON.parse(raw);
  } catch { /* ignore */ }
  return {};
}

/** Save audio settings to localStorage. */
function saveAudioSettings(partial: Record<string, unknown>) {
  try {
    const existing = loadAudioSettings();
    localStorage.setItem('annex:audioSettings', JSON.stringify({ ...existing, ...partial }));
  } catch { /* ignore */ }
}

const saved = loadAudioSettings();

export const useVoiceStore = create<VoiceState>((set, get) => ({
  voiceToken: null,
  webrtcUrl: null,
  iceServers: [],
  connectedChannelId: null,
  joiningByChannel: {},
  connectionState: 'idle' as ConnectionState,
  connectionError: null,
  callActiveByChannel: {},
  participantsByChannel: {},
  joinErrorByChannel: {},
  deafened: false,
  micMuted: false,
  lastFailedChannelId: null,
  activeJoinRequestId: 0,
  joiningAnyCall: false,
  micToggleError: null,

  inputDeviceId: (saved.inputDeviceId as string) ?? null,
  outputDeviceId: (saved.outputDeviceId as string) ?? null,
  outputVolume: (saved.outputVolume as number) ?? 100,
  cameraDeviceId: (saved.cameraDeviceId as string) ?? null,

  voiceSessionDisabled: false,
  voiceSessionDisabledReason: null,
  setVoiceSessionDisabled: (disabled, reason) => set({
    voiceSessionDisabled: disabled,
    voiceSessionDisabledReason: reason ?? null,
  }),

  joinCall: async (pseudonymId, channelId) => {
    const requestId = get().activeJoinRequestId + 1;
    set((s) => ({
      activeJoinRequestId: requestId,
      joiningAnyCall: true,
      joiningByChannel: { ...s.joiningByChannel, [channelId]: true },
      joinErrorByChannel: { ...s.joinErrorByChannel, [channelId]: null },
      // Clear stale failure state on fresh join attempt
      lastFailedChannelId: null,
      connectionError: null,
      micToggleError: null,
    }));
    try {
      const { token, url, ice_servers } = await api.joinVoice(pseudonymId, channelId, 30_000);
      // Only commit if this is still the latest join request
      if (get().activeJoinRequestId !== requestId) return;
      set((s) => ({
        voiceToken: token,
        webrtcUrl: url,
        iceServers: ice_servers ?? [],
        connectedChannelId: channelId,
        joiningByChannel: { ...s.joiningByChannel, [channelId]: false },
        joinErrorByChannel: { ...s.joinErrorByChannel, [channelId]: null },
        connectionState: 'connecting' as ConnectionState,
        connectionError: null,
        joiningAnyCall: false,
        lastFailedChannelId: null,
      }));
    } catch (error) {
      // Only commit if this is still the latest join request
      if (get().activeJoinRequestId !== requestId) return;
      const details = getJoinErrorMessage(error);
      set((s) => ({
        joiningByChannel: { ...s.joiningByChannel, [channelId]: false },
        joinErrorByChannel: { ...s.joinErrorByChannel, [channelId]: details },
        joiningAnyCall: false,
      }));
    }
  },

  leaveCall: async (pseudonymId) => {
    const { connectedChannelId } = get();
    if (connectedChannelId) {
      try {
        await api.leaveVoice(pseudonymId, connectedChannelId);
      } catch { /* best effort */ }
    }
    set((s) => ({
      voiceToken: null,
      webrtcUrl: null,
      iceServers: [],
      connectedChannelId: null,
      deafened: false,
      connectionState: 'idle' as ConnectionState,
      connectionError: null,
      lastFailedChannelId: null,
      micToggleError: null,
      // Clear call-active status for the channel we just left
      callActiveByChannel: connectedChannelId
        ? { ...s.callActiveByChannel, [connectedChannelId]: false }
        : s.callActiveByChannel,
    }));
  },

  toggleDeafen: () => set((s) => ({ deafened: !s.deafened })),
  toggleMicMuted: () => set((s) => ({ micMuted: !s.micMuted })),
  setMicMuted: (muted) => set({ micMuted: muted }),

  setInputDevice: (deviceId) => {
    set({ inputDeviceId: deviceId });
    saveAudioSettings({ inputDeviceId: deviceId });
  },
  setOutputDevice: (deviceId) => {
    set({ outputDeviceId: deviceId });
    saveAudioSettings({ outputDeviceId: deviceId });
  },
  setOutputVolume: (vol) => {
    set({ outputVolume: vol });
    saveAudioSettings({ outputVolume: vol });
  },
  setCameraDevice: (deviceId) => {
    set({ cameraDeviceId: deviceId });
    saveAudioSettings({ cameraDeviceId: deviceId });
  },
  setConnectionState: (state, error = null) => {
    set({ connectionState: state, connectionError: error ?? null });
    // If the room failed, preserve channel ID for error display, then clear session
    if (state === 'failed') {
      const { connectedChannelId } = get();
      set({
        voiceToken: null,
        webrtcUrl: null,
        iceServers: [],
        connectedChannelId: null,
        lastFailedChannelId: connectedChannelId,
      });
    }
  },
  checkCallActive: async (pseudonymId, channelId) => {
    try {
      const status = await api.getVoiceStatus(pseudonymId, channelId);
      set((s) => ({
        callActiveByChannel: { ...s.callActiveByChannel, [channelId]: status.active },
        participantsByChannel: { ...s.participantsByChannel, [channelId]: status.participant_ids },
      }));
    } catch {
      // A failed poll clears the "is a call running" flag, because that is what
      // it could not confirm — but it deliberately LEAVES the roster alone.
      //
      // Emptying it here made every participant tile vanish on a single
      // dropped request and reappear on the next poll ten seconds later, so a
      // call visibly emptied itself and refilled. A request that did not
      // arrive is not evidence that everyone left; the last known roster is a
      // better answer than "nobody".
      set((s) => ({
        callActiveByChannel: { ...s.callActiveByChannel, [channelId]: false },
      }));
    }
  },
  isCallActive: (channelId) => {
    return get().callActiveByChannel[channelId] ?? false;
  },
  getJoinError: (channelId) => {
    return get().joinErrorByChannel[channelId] ?? null;
  },
  isJoining: (channelId) => {
    return get().joiningByChannel[channelId] ?? false;
  },
  clearChannelCallState: (channelId) => {
    // Never clear the channel whose call you are actually in.
    //
    // This is called when the active channel changes, to stop the next
    // channel inheriting the previous one's status and errors — which is
    // right, except that "the channel you were looking at" and "the channel
    // whose call you are in" are different things. A user in a call in #standup
    // who clicks #general to read something is still in the call: the panel
    // stays mounted and keeps rendering tiles. Clearing here wiped that
    // call's roster and every remote tile lost its name, for the same reason
    // as the poll that used to stop on connect — the roster was discarded by
    // code reasoning about a different channel.
    if (get().connectedChannelId === channelId) return;
    set((s) => {
      const restActive = Object.fromEntries(Object.entries(s.callActiveByChannel).filter(([k]) => k !== channelId));
      const restRoster = Object.fromEntries(Object.entries(s.participantsByChannel).filter(([k]) => k !== channelId));
      const restErrors = Object.fromEntries(Object.entries(s.joinErrorByChannel).filter(([k]) => k !== channelId));
      const restJoining = Object.fromEntries(Object.entries(s.joiningByChannel).filter(([k]) => k !== channelId));
      return {
        callActiveByChannel: restActive,
        participantsByChannel: restRoster,
        joinErrorByChannel: restErrors,
        joiningByChannel: restJoining,
        // Clear failure state when switching away from the failed channel
        lastFailedChannelId: s.lastFailedChannelId === channelId ? null : s.lastFailedChannelId,
        connectionError: s.lastFailedChannelId === channelId ? null : s.connectionError,
      };
    });
  },
  handleUnexpectedDisconnect: (errorMessage?: string) => {
    const { connectedChannelId } = get();
    set({
      voiceToken: null,
      webrtcUrl: null,
      iceServers: [],
      connectedChannelId: null,
      lastFailedChannelId: connectedChannelId,
      connectionState: 'failed' as ConnectionState,
      connectionError: errorMessage ?? 'Voice disconnected unexpectedly.',
    });
  },
  forceReset: () => {
    set({
      voiceToken: null,
      webrtcUrl: null,
      iceServers: [],
      connectedChannelId: null,
      joiningByChannel: {},
      connectionState: 'idle' as ConnectionState,
      connectionError: null,
      callActiveByChannel: {},
      participantsByChannel: {},
      joinErrorByChannel: {},
      deafened: false,
      micMuted: false,
      lastFailedChannelId: null,
      joiningAnyCall: false,
      micToggleError: null,
      voiceSessionDisabled: false,
      voiceSessionDisabledReason: null,
    });
  },
  dismissConnectionError: () => {
    set({ lastFailedChannelId: null, connectionError: null });
  },
  setMicToggleError: (message) => {
    set({ micToggleError: message });
  },
  clearMicToggleError: () => {
    set({ micToggleError: null });
  },
}));
