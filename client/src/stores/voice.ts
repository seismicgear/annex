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

/** One transcribed utterance from a voice call. */
export interface TranscriptLine {
  channelId: string;
  speakerPseudonym: string;
  text: string;
  /** Client receipt time. The server frame carries no timestamp. */
  at: number;
}

/**
 * How many caption lines to keep.
 *
 * Captions are a live aid, not a transcript archive — nothing scrolls back
 * through them, and an hour-long call would otherwise accumulate thousands
 * of lines in memory for a strip that shows the last few.
 */
export const MAX_TRANSCRIPT_LINES = 50;

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

  /**
   * Live speech-to-text for the call in progress, oldest first.
   *
   * The server has always produced these. `whisper.cpp` transcribes the call
   * audio, `OutgoingMessage::Transcription` carries each line over the
   * WebSocket to every participant, and startup even reports whether STT is
   * ready — and nothing in the client read the frame. It arrived, passed
   * validation, matched none of the branches in `handleFrame`, and was
   * dropped. A whole subsystem, correct at every layer, rendering nowhere.
   *
   * Capped at [`MAX_TRANSCRIPT_LINES`]: a long call would otherwise grow this
   * without bound, and nobody scrolls back through captions.
   */
  transcripts: TranscriptLine[];

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
  /** Record one transcribed line. Ignored unless it belongs to this call. */
  appendTranscript: (line: TranscriptLine) => void;
  /** Get join error for a specific channel. */
  getJoinError: (channelId: string) => JoinError | null;
  /** Check if a join is in progress for a specific channel. */
  isJoining: (channelId: string) => boolean;
  /** Clear cached call status for a channel (used on channel switch). */
  clearChannelCallState: (channelId: string) => void;
  /** Handle an unexpected disconnect — clears session state and records a user-visible error. */
  handleUnexpectedDisconnect: (errorMessage?: string) => void;
  /**
   * Server-side voice readiness, or null before it has been asked for.
   *
   * The panel used to learn about an unprovisioned server only by FAILING a
   * join: `setupHint` was populated from the join error. So an operator who
   * had not configured WebRTC got a live-looking "Create Call" button, and
   * the explanation the server had ready all along appeared only after the
   * user pressed it and it failed.
   */
  voiceConfig: api.VoiceConfigStatus | null;
  /** Whether the readiness check has been attempted, and how it went. */
  voiceConfigStatus: 'idle' | 'loading' | 'ready' | 'error';
  /**
   * Fetch server voice readiness once per server. Cheap and idempotent:
   * subsequent calls no-op while loading or once resolved. `force` re-asks,
   * which matters after an admin flips the policy toggle.
   */
  loadVoiceConfig: (force?: boolean) => Promise<void>;
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
  transcripts: [],

  voiceConfig: null,
  voiceConfigStatus: 'idle' as const,

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
      // Captions belong to one call. Carrying the last call's lines into the
      // next one would put words in a new room that were said in another.
      transcripts: [],
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
      transcripts: [],
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
  loadVoiceConfig: async (force = false) => {
    const { voiceConfigStatus } = get();
    if (!force && (voiceConfigStatus === 'loading' || voiceConfigStatus === 'ready')) return;
    set({ voiceConfigStatus: 'loading' });
    try {
      const config = await api.getVoiceConfigStatus();
      set({ voiceConfig: config, voiceConfigStatus: 'ready' });
    } catch {
      // Non-fatal, and deliberately NOT treated as "voice is broken": a failed
      // readiness check says nothing about the server's configuration. The
      // panel leaves the button as the permissions checks left it, and a join
      // that does fail still surfaces the server's own hint.
      set({ voiceConfig: null, voiceConfigStatus: 'error' });
    }
  },
  forceReset: () => {
    set({
      voiceConfig: null,
      voiceConfigStatus: 'idle' as const,
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
      transcripts: [],
      voiceSessionDisabled: false,
      voiceSessionDisabledReason: null,
    });
  },
  dismissConnectionError: () => {
    set({ lastFailedChannelId: null, connectionError: null });
  },
  appendTranscript: (line) => {
    set((s) => {
      // A line for a channel this client is not in a call on is not this
      // call's. The server sends transcripts to call participants, but a
      // channel switch mid-call and a late-arriving frame can cross, and
      // captions attributed to the wrong room are worse than none.
      if (!s.connectedChannelId || line.channelId !== s.connectedChannelId) return s;
      const next = [...s.transcripts, line];
      return {
        transcripts:
          next.length > MAX_TRANSCRIPT_LINES ? next.slice(next.length - MAX_TRANSCRIPT_LINES) : next,
      };
    });
  },
  setMicToggleError: (message) => {
    set({ micToggleError: message });
  },
  clearMicToggleError: () => {
    set({ micToggleError: null });
  },
}));
