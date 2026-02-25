/**
 * Voice store — manages persistent voice/video call state.
 *
 * Call state lives here (not in component useState) so it survives
 * tab switches, channel switches, and view changes — matching the
 * Discord pattern where the call stays connected in the background.
 */

import { create } from 'zustand';
import * as api from '@/lib/api';

function getJoinErrorMessage(error: unknown): string {
  if (error instanceof api.ApiError) {
    const body = error.message?.trim();
    if (!body) return `Failed to join voice (${error.status})`;

    try {
      const parsed = JSON.parse(body) as { error?: string; message?: string };
      return parsed.error ?? parsed.message ?? body;
    } catch {
      return body;
    }
  }

  if (error instanceof Error && error.message) {
    return error.message;
  }

  return 'Failed to join voice';
}

export interface VoiceState {
  /** LiveKit access token for the current session. */
  voiceToken: string | null;
  /** LiveKit server URL. */
  livekitUrl: string | null;
  /** Channel ID the call is connected to. */
  connectedChannelId: string | null;
  /** Whether a join is in progress. */
  joining: boolean;
  /** Whether a call is active on the current channel (for Join vs Create). */
  callActive: boolean;
  /** Most recent join failure shown in the UI. */
  lastJoinError: string | null;
  /** Whether the user has self-deafened (output muted). */
  deafened: boolean;

  /** Audio settings persisted across sessions. */
  inputDeviceId: string | null;
  outputDeviceId: string | null;
  inputVolume: number;   // 0–100
  outputVolume: number;  // 0–100

  /** Join a voice call on the given channel. */
  joinCall: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Leave the current voice call. */
  leaveCall: (pseudonymId: string) => Promise<void>;
  /** Toggle self-deafen state. */
  toggleDeafen: () => void;
  /** Update audio settings. */
  setInputDevice: (deviceId: string | null) => void;
  setOutputDevice: (deviceId: string | null) => void;
  setInputVolume: (vol: number) => void;
  setOutputVolume: (vol: number) => void;
  /** Check if a call is active on a channel (for polling). */
  checkCallActive: (pseudonymId: string, channelId: string) => Promise<void>;
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
  connectedChannelId: null,
  joining: false,
  callActive: false,
  lastJoinError: null,
  deafened: false,

  inputDeviceId: (saved.inputDeviceId as string) ?? null,
  outputDeviceId: (saved.outputDeviceId as string) ?? null,
  inputVolume: (saved.inputVolume as number) ?? 100,
  outputVolume: (saved.outputVolume as number) ?? 100,

  joinCall: async (pseudonymId, channelId) => {
    set({ joining: true, lastJoinError: null });
    try {
      const { token, url } = await api.joinVoice(pseudonymId, channelId);
      set({
        voiceToken: token,
        livekitUrl: url,
        connectedChannelId: channelId,
        joining: false,
        lastJoinError: null,
      });
    } catch (error) {
      set({ joining: false, lastJoinError: getJoinErrorMessage(error) });
    }
  },

  leaveCall: async (pseudonymId) => {
    const { connectedChannelId } = get();
    if (connectedChannelId) {
      try {
        await api.leaveVoice(pseudonymId, connectedChannelId);
      } catch { /* best effort */ }
    }
    set({
      voiceToken: null,
      livekitUrl: null,
      connectedChannelId: null,
      lastJoinError: null,
      deafened: false,
    });
  },

  toggleDeafen: () => set((s) => ({ deafened: !s.deafened })),

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
  checkCallActive: async (pseudonymId, channelId) => {
    try {
      const status = await api.getVoiceStatus(pseudonymId, channelId);
      set({ callActive: status.active });
    } catch {
      set({ callActive: false });
    }
  },
}));
