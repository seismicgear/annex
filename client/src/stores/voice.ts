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

export interface VoiceState {
  /** LiveKit access token for the current session. */
  voiceToken: string | null;
  /** LiveKit server URL. */
  livekitUrl: string | null;
  /** ICE (STUN/TURN) servers for WebRTC NAT traversal. */
  iceServers: api.IceServerConfig[];
  /** Channel ID the call is connected to. */
  connectedChannelId: string | null;
  /** Whether a join is in progress. */
  joining: boolean;
  /** Per-channel call-active status (keyed by channelId). */
  callActiveByChannel: Record<string, boolean>;
  /** Per-channel join error (keyed by channelId). */
  joinErrorByChannel: Record<string, JoinError | null>;
  /** Whether the user has self-deafened (output muted). */
  deafened: boolean;
  /** Whether the local microphone is muted (shared source of truth). */
  micMuted: boolean;

  /** Audio settings persisted across sessions. */
  inputDeviceId: string | null;
  outputDeviceId: string | null;
  inputVolume: number;   // 0–100
  outputVolume: number;  // 0–100
  /** Camera device ID (persisted). */
  cameraDeviceId: string | null;

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
  setInputVolume: (vol: number) => void;
  setOutputVolume: (vol: number) => void;
  setCameraDevice: (deviceId: string | null) => void;
  /** Check if a call is active on a channel (for polling). */
  checkCallActive: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Get call-active status for a specific channel. */
  isCallActive: (channelId: string) => boolean;
  /** Get join error for a specific channel. */
  getJoinError: (channelId: string) => JoinError | null;
  /** Clear cached call status for a channel (used on channel switch). */
  clearChannelCallState: (channelId: string) => void;
  /** Force-clear all voice session state (used by server switching). */
  forceReset: () => void;
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
  livekitUrl: null,
  iceServers: [],
  connectedChannelId: null,
  joining: false,
  callActiveByChannel: {},
  joinErrorByChannel: {},
  deafened: false,
  micMuted: false,

  inputDeviceId: (saved.inputDeviceId as string) ?? null,
  outputDeviceId: (saved.outputDeviceId as string) ?? null,
  inputVolume: (saved.inputVolume as number) ?? 100,
  outputVolume: (saved.outputVolume as number) ?? 100,
  cameraDeviceId: (saved.cameraDeviceId as string) ?? null,

  joinCall: async (pseudonymId, channelId) => {
    set((s) => ({
      joining: true,
      joinErrorByChannel: { ...s.joinErrorByChannel, [channelId]: null },
    }));
    try {
      const { token, url, ice_servers } = await api.joinVoice(pseudonymId, channelId);
      set((s) => ({
        voiceToken: token,
        livekitUrl: url,
        iceServers: ice_servers ?? [],
        connectedChannelId: channelId,
        joining: false,
        joinErrorByChannel: { ...s.joinErrorByChannel, [channelId]: null },
      }));
    } catch (error) {
      const details = getJoinErrorMessage(error);
      set((s) => ({
        joining: false,
        joinErrorByChannel: { ...s.joinErrorByChannel, [channelId]: details },
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
      livekitUrl: null,
      iceServers: [],
      connectedChannelId: null,
      deafened: false,
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
  setInputVolume: (vol) => {
    set({ inputVolume: vol });
    saveAudioSettings({ inputVolume: vol });
  },
  setOutputVolume: (vol) => {
    set({ outputVolume: vol });
    saveAudioSettings({ outputVolume: vol });
  },
  setCameraDevice: (deviceId) => {
    set({ cameraDeviceId: deviceId });
    saveAudioSettings({ cameraDeviceId: deviceId });
  },
  checkCallActive: async (pseudonymId, channelId) => {
    try {
      const status = await api.getVoiceStatus(pseudonymId, channelId);
      set((s) => ({
        callActiveByChannel: { ...s.callActiveByChannel, [channelId]: status.active },
      }));
    } catch {
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
  clearChannelCallState: (channelId) => {
    set((s) => {
      const { [channelId]: _active, ...restActive } = s.callActiveByChannel;
      const { [channelId]: _error, ...restErrors } = s.joinErrorByChannel;
      return { callActiveByChannel: restActive, joinErrorByChannel: restErrors };
    });
  },
  forceReset: () => {
    set({
      voiceToken: null,
      livekitUrl: null,
      iceServers: [],
      connectedChannelId: null,
      joining: false,
      callActiveByChannel: {},
      joinErrorByChannel: {},
      deafened: false,
      micMuted: false,
    });
  },
}));
