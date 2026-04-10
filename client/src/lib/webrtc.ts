/**
 * Native WebRTC session manager.
 *
 * Connects to the Annex native Rust SFU via WebSocket signaling
 * using direct RTCPeerConnection for voice, video, and screen sharing.
 *
 * The SFU is audio-first: it creates 2 outbound audio tracks per peer
 * (forwarded peer audio + agent TTS). Video tracks are offered by the
 * client and accepted by the SFU but not forwarded to other peers yet.
 */

// ── Track source enum ──

export const TrackSource = {
  Microphone: 'microphone',
  Camera: 'camera',
  ScreenShare: 'screen_share',
} as const;
export type TrackSource = (typeof TrackSource)[keyof typeof TrackSource];

// ── Connection state ──

export const ConnectionState = {
  Connected: 'connected',
  Connecting: 'connecting',
  Reconnecting: 'reconnecting',
  Disconnected: 'disconnected',
} as const;
export type NativeConnectionState = (typeof ConnectionState)[keyof typeof ConnectionState];

// ── Track publication (compatible with Tauri media restore hook) ──

export interface TrackPublication {
  source: TrackSource;
  track: { mediaStreamTrack: MediaStreamTrack } | null;
  isMuted: boolean;
}

// ── Remote audio track info ──

export interface RemoteAudioTrack {
  id: string;
  track: MediaStreamTrack;
  stream: MediaStream;
}

// ── Signaling callbacks injected by the consumer ──

export interface SignalingCallbacks {
  sendOffer(channelId: string, sdp: string): void;
  sendIceCandidate(
    channelId: string,
    candidate: string,
    sdpMid: string | null,
    sdpMLineIndex: number | null,
  ): void;
}

// ── WebRtcSession ──

/**
 * Manages a single RTCPeerConnection to the Annex SFU for one voice call.
 *
 * Provides a localParticipant-compatible interface so VoicePanel inner
 * components can use it with minimal changes.
 */
export class WebRtcSession {
  // ── Public state ──

  connectionState: NativeConnectionState = ConnectionState.Disconnected;
  remoteAudioTracks: RemoteAudioTrack[] = [];
  isSpeaking = false;

  // ── Callbacks ──

  onConnectionStateChange?: (state: NativeConnectionState) => void;
  /** Fires when a remote audio track is added or removed. */
  onRemoteTracksChanged?: () => void;
  /** Fires when local publication state changes (mic/camera/screen). */
  onLocalTrackChanged?: () => void;

  // ── Private ──

  private pc: RTCPeerConnection | null = null;
  private channelId: string;
  private _identity: string;
  private signaling: SignalingCallbacks;
  private iceServers: RTCIceServer[];

  // Local tracks
  private micTrack: MediaStreamTrack | null = null;
  private micSender: RTCRtpSender | null = null;
  private cameraTrack: MediaStreamTrack | null = null;
  private cameraSender: RTCRtpSender | null = null;
  private screenTrack: MediaStreamTrack | null = null;
  private screenSender: RTCRtpSender | null = null;

  // Speaking detection
  private audioContext: AudioContext | null = null;
  private analyser: AnalyserNode | null = null;
  private speakingInterval: ReturnType<typeof setInterval> | null = null;

  // Pending ICE candidates received before remote description is set
  private pendingCandidates: RTCIceCandidateInit[] = [];
  private remoteDescriptionSet = false;

  constructor(
    iceServers: RTCIceServer[],
    channelId: string,
    identity: string,
    signaling: SignalingCallbacks,
  ) {
    this.iceServers = iceServers;
    this.channelId = channelId;
    this._identity = identity;
    this.signaling = signaling;
  }

  // ── Getters (localParticipant-compatible) ──

  get identity(): string {
    return this._identity;
  }

  get isMicrophoneEnabled(): boolean {
    return this.micTrack !== null && this.micTrack.readyState === 'live' && this.micTrack.enabled;
  }

  get isCameraEnabled(): boolean {
    return this.cameraTrack !== null && this.cameraTrack.readyState === 'live' && this.cameraTrack.enabled;
  }

  get isScreenShareEnabled(): boolean {
    return this.screenTrack !== null && this.screenTrack.readyState === 'live' && this.screenTrack.enabled;
  }

  /**
   * Track publications map compatible with the Tauri media restore hook.
   * Keys are source strings, values have `.source`, `.track.mediaStreamTrack`, `.isMuted`.
   */
  get trackPublications(): Map<string, TrackPublication> {
    const pubs = new Map<string, TrackPublication>();
    if (this.micTrack) {
      pubs.set(TrackSource.Microphone, {
        source: TrackSource.Microphone,
        track: { mediaStreamTrack: this.micTrack },
        isMuted: !this.micTrack.enabled,
      });
    }
    if (this.cameraTrack) {
      pubs.set(TrackSource.Camera, {
        source: TrackSource.Camera,
        track: { mediaStreamTrack: this.cameraTrack },
        isMuted: !this.cameraTrack.enabled,
      });
    }
    if (this.screenTrack) {
      pubs.set(TrackSource.ScreenShare, {
        source: TrackSource.ScreenShare,
        track: { mediaStreamTrack: this.screenTrack },
        isMuted: !this.screenTrack.enabled,
      });
    }
    return pubs;
  }

  // ── Lifecycle ──

  /**
   * Create the peer connection, acquire the microphone, and send an SDP offer.
   * The caller must wire up handleAnswer/handleIceCandidate from WS messages.
   */
  async connect(): Promise<void> {
    this.setConnectionState(ConnectionState.Connecting);
    this.remoteDescriptionSet = false;
    this.pendingCandidates = [];

    const pc = new RTCPeerConnection({
      iceServers: this.iceServers,
      bundlePolicy: 'max-bundle',
    });
    this.pc = pc;

    // Track connection state
    pc.onconnectionstatechange = () => {
      switch (pc.connectionState) {
        case 'new':
        case 'connecting':
          this.setConnectionState(ConnectionState.Connecting);
          break;
        case 'connected':
          this.setConnectionState(ConnectionState.Connected);
          break;
        case 'disconnected':
          this.setConnectionState(ConnectionState.Disconnected);
          break;
        case 'failed':
          this.setConnectionState(ConnectionState.Disconnected);
          break;
        case 'closed':
          this.setConnectionState(ConnectionState.Disconnected);
          break;
      }
    };

    // Trickle ICE — send candidates as they're discovered
    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.signaling.sendIceCandidate(
          this.channelId,
          event.candidate.candidate,
          event.candidate.sdpMid,
          event.candidate.sdpMLineIndex,
        );
      }
    };

    // Remote tracks from the SFU (peer audio + agent audio)
    pc.ontrack = (event) => {
      const track = event.track;
      const stream = event.streams[0] ?? new MediaStream([track]);

      if (track.kind === 'audio') {
        const remoteTrack: RemoteAudioTrack = {
          id: track.id,
          track,
          stream,
        };
        this.remoteAudioTracks = [...this.remoteAudioTracks, remoteTrack];
        this.onRemoteTracksChanged?.();

        track.onended = () => {
          this.remoteAudioTracks = this.remoteAudioTracks.filter((t) => t.id !== track.id);
          this.onRemoteTracksChanged?.();
        };
      }
    };

    // Acquire microphone and add to peer connection
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true },
      });
      this.micTrack = stream.getAudioTracks()[0];
      this.micSender = pc.addTrack(this.micTrack, stream);
      this.startSpeakingDetection(stream);
    } catch (err) {
      // Mic not available — still connect for listen-only
      console.warn('[webrtc] microphone unavailable, connecting in listen-only mode:', err);
      // Add a recvonly audio transceiver so the SFU can still send us audio
      pc.addTransceiver('audio', { direction: 'recvonly' });
    }

    // Create and send offer
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    this.signaling.sendOffer(this.channelId, offer.sdp!);
  }

  /** Handle an SDP answer from the SFU. */
  async handleAnswer(sdp: string): Promise<void> {
    if (!this.pc) return;
    await this.pc.setRemoteDescription(new RTCSessionDescription({ type: 'answer', sdp }));
    this.remoteDescriptionSet = true;

    // Flush any ICE candidates that arrived before the answer
    for (const candidate of this.pendingCandidates) {
      await this.pc.addIceCandidate(new RTCIceCandidate(candidate));
    }
    this.pendingCandidates = [];
  }

  /** Handle an ICE candidate from the SFU. */
  async handleIceCandidate(init: RTCIceCandidateInit): Promise<void> {
    if (!this.pc) return;
    if (!this.remoteDescriptionSet) {
      // Buffer until remote description is set
      this.pendingCandidates.push(init);
      return;
    }
    await this.pc.addIceCandidate(new RTCIceCandidate(init));
  }

  /** Tear down the peer connection and all local tracks. */
  disconnect(): void {
    this.stopSpeakingDetection();

    if (this.micTrack) {
      this.micTrack.stop();
      this.micTrack = null;
      this.micSender = null;
    }
    if (this.cameraTrack) {
      this.cameraTrack.stop();
      this.cameraTrack = null;
      this.cameraSender = null;
    }
    if (this.screenTrack) {
      this.screenTrack.stop();
      this.screenTrack = null;
      this.screenSender = null;
    }

    if (this.pc) {
      this.pc.onconnectionstatechange = null;
      this.pc.onicecandidate = null;
      this.pc.ontrack = null;
      this.pc.close();
      this.pc = null;
    }

    this.remoteAudioTracks = [];
    this.remoteDescriptionSet = false;
    this.pendingCandidates = [];
    this.setConnectionState(ConnectionState.Disconnected);
  }

  // ── Media controls ──

  async setMicrophoneEnabled(enabled: boolean, opts?: { deviceId?: string }): Promise<void> {
    if (!this.pc) throw new Error('Not connected');

    if (enabled) {
      // Acquire new mic track (respects device selection)
      const constraints: MediaTrackConstraints = {
        echoCancellation: true,
        noiseSuppression: true,
      };
      if (opts?.deviceId) constraints.deviceId = opts.deviceId;

      const stream = await navigator.mediaDevices.getUserMedia({ audio: constraints });
      const newTrack = stream.getAudioTracks()[0];

      // Replace or add the track on the peer connection
      if (this.micSender) {
        await this.micSender.replaceTrack(newTrack);
      } else {
        this.micSender = this.pc.addTrack(newTrack, stream);
      }

      // Stop old track if it exists
      if (this.micTrack && this.micTrack !== newTrack) {
        this.micTrack.stop();
      }
      this.micTrack = newTrack;
      this.startSpeakingDetection(stream);
    } else {
      // Mute: stop the track and clear the sender
      if (this.micTrack) {
        this.micTrack.stop();
        this.micTrack = null;
      }
      if (this.micSender) {
        await this.micSender.replaceTrack(null);
      }
      this.stopSpeakingDetection();
      this.isSpeaking = false;
    }
    this.onLocalTrackChanged?.();
  }

  async setCameraEnabled(enabled: boolean, opts?: { deviceId?: string }): Promise<void> {
    if (!this.pc) throw new Error('Not connected');

    if (enabled) {
      const constraints: MediaTrackConstraints = {
        width: { ideal: 640 },
        height: { ideal: 480 },
        frameRate: { ideal: 24 },
      };
      if (opts?.deviceId) constraints.deviceId = opts.deviceId;

      const stream = await navigator.mediaDevices.getUserMedia({ video: constraints });
      const newTrack = stream.getVideoTracks()[0];

      if (this.cameraSender) {
        await this.cameraSender.replaceTrack(newTrack);
      } else {
        this.cameraSender = this.pc.addTrack(newTrack, stream);
      }

      if (this.cameraTrack && this.cameraTrack !== newTrack) {
        this.cameraTrack.stop();
      }
      this.cameraTrack = newTrack;
    } else {
      if (this.cameraTrack) {
        this.cameraTrack.stop();
        this.cameraTrack = null;
      }
      if (this.cameraSender) {
        await this.cameraSender.replaceTrack(null);
      }
    }
    this.onLocalTrackChanged?.();
  }

  async setScreenShareEnabled(enabled: boolean): Promise<void> {
    if (!this.pc) throw new Error('Not connected');

    if (enabled) {
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: true,
        audio: false,
      });
      const newTrack = stream.getVideoTracks()[0];

      // Listen for browser-level stop (user clicks "Stop Sharing" in browser chrome)
      newTrack.onended = () => {
        this.screenTrack = null;
        if (this.screenSender) {
          this.screenSender.replaceTrack(null).catch(() => {});
        }
        this.onLocalTrackChanged?.();
      };

      if (this.screenSender) {
        await this.screenSender.replaceTrack(newTrack);
      } else {
        this.screenSender = this.pc.addTrack(newTrack, stream);
      }

      if (this.screenTrack && this.screenTrack !== newTrack) {
        this.screenTrack.stop();
      }
      this.screenTrack = newTrack;
    } else {
      if (this.screenTrack) {
        this.screenTrack.stop();
        this.screenTrack = null;
      }
      if (this.screenSender) {
        await this.screenSender.replaceTrack(null);
      }
    }
    this.onLocalTrackChanged?.();
  }

  // ── Speaking detection ──

  private startSpeakingDetection(stream: MediaStream): void {
    this.stopSpeakingDetection();

    try {
      this.audioContext = new AudioContext();
      const source = this.audioContext.createMediaStreamSource(stream);
      this.analyser = this.audioContext.createAnalyser();
      this.analyser.fftSize = 256;
      source.connect(this.analyser);

      const dataArray = new Uint8Array(this.analyser.frequencyBinCount);

      this.speakingInterval = setInterval(() => {
        if (!this.analyser) return;
        this.analyser.getByteFrequencyData(dataArray);
        // Average energy across frequency bins
        let sum = 0;
        for (let i = 0; i < dataArray.length; i++) sum += dataArray[i];
        const avg = sum / dataArray.length;
        // Threshold: treat > 20 as speaking (tuned for voice frequencies)
        this.isSpeaking = avg > 20;
      }, 100);
    } catch {
      // AudioContext not available (e.g. test environment)
    }
  }

  private stopSpeakingDetection(): void {
    if (this.speakingInterval) {
      clearInterval(this.speakingInterval);
      this.speakingInterval = null;
    }
    if (this.audioContext) {
      this.audioContext.close().catch(() => {});
      this.audioContext = null;
    }
    this.analyser = null;
  }

  // ── Internals ──

  private setConnectionState(state: NativeConnectionState): void {
    if (this.connectionState === state) return;
    this.connectionState = state;
    this.onConnectionStateChange?.(state);
  }
}
