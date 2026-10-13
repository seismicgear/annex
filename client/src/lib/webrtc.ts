/**
 * Native WebRTC session manager.
 *
 * Connects to the Annex native Rust SFU via WebSocket signaling
 * using direct RTCPeerConnection for voice, video, and screen sharing.
 *
 * The SFU forwards each participant's audio, camera and screen separately.
 * Camera and screen slots are negotiated before either capture is enabled.
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

/**
 * Pull the sender's pseudonym out of a track id.
 *
 * The SFU names each outbound track `audio-<pseudonym>` / `video-<pseudonym>`
 * for the peer whose media it carries, and that reaches the browser as the
 * MSID. It is the only per-track attribution available: before it existed the
 * client had a bag of anonymous tracks and every remote tile rendered the
 * literal string "Participant".
 *
 * Returns null for the agent (TTS) track and anything else not matching, so an
 * unrecognised track degrades to an unnamed tile rather than a wrong name.
 */
export function senderFromTrackId(trackId: string): string | null {
  const m = /^(?:audio|video|screen)-(.+)$/.exec(trackId);
  return m ? m[1] : null;
}

export interface RemoteAudioTrack {
  id: string;
  /** Pseudonym of the peer whose media this carries, if attributable. */
  sender: string | null;
  track: MediaStreamTrack;
  stream: MediaStream;
}

export interface RemoteVideoTrack {
  id: string;
  /** Pseudonym of the peer whose media this carries, if attributable. */
  sender: string | null;
  track: MediaStreamTrack;
  stream: MediaStream;
}

// ── Signaling callbacks injected by the consumer ──

export interface SignalingCallbacks {
  sendOffer(channelId: string, sdp: string): void;
  /** Answer an offer the SERVER initiated — see `handleRemoteOffer`. */
  sendAnswer(channelId: string, sdp: string): void;
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
  /** Remote camera/video tracks forwarded by the SFU (one per remote sender). */
  remoteVideoTracks: RemoteVideoTrack[] = [];
  isSpeaking = false;
  /** Capture failures can occur after getUserMedia has already succeeded. */
  localMediaError: string | null = null;

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
  /** Newer user intent supersedes pending permission/device requests. */
  private mediaRequests: Record<TrackSource, number> = {
    microphone: 0, camera: 0, screen_share: 0,
  };
  /** Serialize sender changes while allowing captures to resolve independently. */
  private mediaUpdates: Record<TrackSource, Promise<void>> = {
    microphone: Promise.resolve(), camera: Promise.resolve(), screen_share: Promise.resolve(),
  };
  /** Captures acquired but not committed to a sender yet. */
  private pendingCaptures: Record<TrackSource, Set<MediaStream>> = {
    microphone: new Set(), camera: new Set(), screen_share: new Set(),
  };
  private interruptedMedia = new Set<TrackSource>();
  private localMediaErrorSource: TrackSource | null = null;

  // Speaking detection
  private audioContext: AudioContext | null = null;
  private analyser: AnalyserNode | null = null;
  private speakingInterval: ReturnType<typeof setInterval> | null = null;

  // Pending ICE candidates received before remote description is set
  private pendingCandidates: RTCIceCandidateInit[] = [];
  private remoteDescriptionSet = false;
  private signalingUpdates: Promise<void> = Promise.resolve();

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

  wasMediaInterrupted(source: TrackSource): boolean {
    return this.interruptedMedia.has(source);
  }

  clearLocalMediaError(): void {
    if (!this.localMediaError) return;
    this.localMediaError = null;
    this.localMediaErrorSource = null;
    this.onLocalTrackChanged?.();
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
  async connect(options?: { microphoneEnabled?: boolean; inputDeviceId?: string | null }): Promise<void> {
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
      if (this.pc !== pc) return;
      const track = event.track;
      const stream = event.streams[0] ?? new MediaStream([track]);

      if (track.kind === 'audio') {
        const remoteTrack: RemoteAudioTrack = {
          id: track.id,
          sender: senderFromTrackId(track.id),
          track,
          stream,
        };
        // Re-enabling an existing m-line can dispatch ontrack again for the
        // same receiver. One publication must have only one playback sink.
        this.remoteAudioTracks = [...this.remoteAudioTracks.filter(t => t.id !== track.id), remoteTrack];
        this.onRemoteTracksChanged?.();

        track.onended = () => {
          if (this.pc !== pc) return;
          this.remoteAudioTracks = this.remoteAudioTracks.filter((t) => t.track !== track);
          this.onRemoteTracksChanged?.();
        };
      } else if (track.kind === 'video') {
        // Remote camera forwarded by the SFU — surface it so the grid renders
        // the real video tile (not a "?" avatar). A muted track id (the SFU
        // sends a placeholder until a peer publishes) still creates the slot;
        // the <video> simply shows nothing until frames arrive.
        const remoteVideo: RemoteVideoTrack = {
          id: track.id,
          sender: senderFromTrackId(track.id),
          track,
          stream,
        };
        this.remoteVideoTracks = [...this.remoteVideoTracks.filter(t => t.id !== track.id), remoteVideo];
        this.onRemoteTracksChanged?.();

        track.onended = () => {
          if (this.pc !== pc) return;
          this.remoteVideoTracks = this.remoteVideoTracks.filter((t) => t.track !== track);
          this.onRemoteTracksChanged?.();
        };
        track.onmute = () => { if (this.pc === pc) this.onRemoteTracksChanged?.(); };
        track.onunmute = () => { if (this.pc === pc) this.onRemoteTracksChanged?.(); };
      }
    };

    // All publishing slots must exist in the initial offer. A later addTrack
    // needs renegotiation; without it screen capture only worked locally.
    // The first video MID is camera and the second is screen, a stable source
    // contract with the SFU even when replaceTrack changes capture devices.
    // Publishing and receiving use distinct m-lines. Reusing a pre-created
    // receive slot leaves its immutable browser-generated track ID in place,
    // losing the SFU's audio/video/screen sender attribution for late joiners.
    // The SFU introduces each inbound publication in its follow-up offer.
    this.micSender = pc.addTransceiver('audio', { direction: 'sendonly' }).sender;
    this.cameraSender = pc.addTransceiver('video', { direction: 'sendonly' }).sender;
    this.screenSender = pc.addTransceiver('video', { direction: 'sendonly' }).sender;

    if (options?.microphoneEnabled !== false) {
      try {
        await this.setMicrophoneEnabled(true,
          options?.inputDeviceId ? { deviceId: options.inputDeviceId } : undefined);
      } catch (err) {
        // Keep the negotiated publishing slot so enabling the microphone later
        // works even when the first permission request was declined.
        console.warn('[webrtc] microphone unavailable, connecting in listen-only mode:', err);
      }
    }
    if (this.pc !== pc) return;

    // Create and send offer
    const offer = await pc.createOffer();
    if (this.pc !== pc) return;
    await pc.setLocalDescription(offer);
    if (this.pc !== pc) return;
    this.signaling.sendOffer(this.channelId, offer.sdp!);
  }

  /** Apply WebSocket signaling in wire order, including asynchronous SDP work. */
  private enqueueSignaling(operation: (pc: RTCPeerConnection) => Promise<void>): Promise<void> {
    const pc = this.pc;
    if (!pc) return Promise.resolve();
    const update = this.signalingUpdates.then(async () => {
      if (this.pc !== pc) return;
      await operation(pc);
    });
    this.signalingUpdates = update.catch(() => {
      if (this.pc === pc) this.setConnectionState(ConnectionState.Disconnected);
    });
    return update;
  }

  private async applyRemoteDescription(pc: RTCPeerConnection, type: RTCSdpType, sdp: string): Promise<void> {
    await pc.setRemoteDescription({ type, sdp });
    if (this.pc !== pc) return;
    this.remoteDescriptionSet = true;
    const candidates = this.pendingCandidates.splice(0);
    for (const candidate of candidates) {
      if (this.pc !== pc) return;
      await pc.addIceCandidate(candidate);
    }
  }

  /**
   * Answer an offer the SERVER initiated.
   *
   * Normal setup is this client offering and the SFU answering. This is the
   * other direction: when somebody joins or leaves the call, the SFU's track
   * set for this connection changes, and adding a track to an established
   * connection requires a fresh offer/answer. Without answering, the new
   * tracks never become live — a call stays frozen at whoever was in it when
   * this peer joined.
   */
  handleRemoteOffer(sdp: string): Promise<void> {
    return this.enqueueSignaling(async (pc) => {
      await this.applyRemoteDescription(pc, 'offer', sdp);
      if (this.pc !== pc) return;
      const answer = await pc.createAnswer();
      if (this.pc !== pc) return;
      await pc.setLocalDescription(answer);
      if (this.pc === pc) this.signaling.sendAnswer(this.channelId, answer.sdp!);
    });
  }

  /** Handle the initial SDP answer before processing any follow-up offers. */
  handleAnswer(sdp: string): Promise<void> {
    return this.enqueueSignaling((pc) => this.applyRemoteDescription(pc, 'answer', sdp));
  }

  /** Handle an ICE candidate from the SFU. */
  handleIceCandidate(init: RTCIceCandidateInit): Promise<void> {
    return this.enqueueSignaling(async (pc) => {
      if (!this.remoteDescriptionSet) this.pendingCandidates.push(init);
      else await pc.addIceCandidate(init);
    });
  }

  /** Tear down the peer connection and all local tracks. */
  disconnect(): void {
    this.stopSpeakingDetection();
    this.interruptedMedia.clear();
    this.localMediaError = null;
    this.localMediaErrorSource = null;
    for (const source of Object.values(TrackSource)) {
      this.mediaRequests[source] += 1;
      for (const stream of this.pendingCaptures[source]) stream.getTracks().forEach((track) => track.stop());
      this.pendingCaptures[source].clear();
    }

    if (this.micTrack) {
      this.micTrack.onended = null;
      this.micTrack.stop();
      this.micTrack = null;
      this.micSender = null;
    }
    if (this.cameraTrack) {
      this.cameraTrack.onended = null;
      this.cameraTrack.stop();
      this.cameraTrack = null;
      this.cameraSender = null;
    }
    if (this.screenTrack) {
      this.screenTrack.onended = null;
      this.screenTrack.stop();
      this.screenTrack = null;
      this.screenSender = null;
    }
    this.micSender = null;
    this.cameraSender = null;
    this.screenSender = null;

    if (this.pc) {
      this.pc.onconnectionstatechange = null;
      this.pc.onicecandidate = null;
      this.pc.ontrack = null;
      this.pc.close();
      this.pc = null;
    }

    this.remoteAudioTracks = [];
    this.remoteVideoTracks = [];
    this.remoteDescriptionSet = false;
    this.pendingCandidates = [];
    this.setConnectionState(ConnectionState.Disconnected);
  }

  // ── Media controls ──

  /** Change a capture atomically; failed replacement keeps the working track. */
  private async updateLocalMedia(
    source: TrackSource,
    enabled: boolean,
    capture: () => Promise<MediaStream>,
  ): Promise<void> {
    const pc = this.pc;
    if (!pc) throw new Error('Not connected');
    const request = ++this.mediaRequests[source];
    // A pending replaceTrack does not yet own a publication, but its device
    // is already recording. Stop it immediately when superseded or muted.
    for (const pending of this.pendingCaptures[source]) pending.getTracks().forEach((track) => track.stop());
    this.pendingCaptures[source].clear();
    const current = () => this.pc === pc && this.mediaRequests[source] === request;
    const getTrack = () => source === TrackSource.Microphone ? this.micTrack
      : source === TrackSource.Camera ? this.cameraTrack : this.screenTrack;
    const setTrack = (track: MediaStreamTrack | null) => {
      if (source === TrackSource.Microphone) this.micTrack = track;
      else if (source === TrackSource.Camera) this.cameraTrack = track;
      else this.screenTrack = track;
    };

    // Turning capture off is immediate, even while an earlier permission
    // prompt or replaceTrack is pending. Its eventual result is invalidated.
    if (!enabled) {
      const previous = getTrack();
      if (previous) { previous.onended = null; previous.stop(); }
      setTrack(null);
      this.interruptedMedia.delete(source);
      if (this.localMediaErrorSource === source) this.clearLocalMediaError();
      if (source === TrackSource.Microphone) {
        this.stopSpeakingDetection();
        this.isSpeaking = false;
      }
      this.onLocalTrackChanged?.();
    }

    const stream = enabled ? await capture() : null;
    const stopCapture = () => {
      stream?.getTracks().forEach((track) => track.stop());
      if (stream) this.pendingCaptures[source].delete(stream);
    };
    if (!current()) { stopCapture(); return; }
    if (stream) this.pendingCaptures[source].add(stream);
    const update = this.mediaUpdates[source].then(async () => {
      if (!current()) { stopCapture(); return; }
      const newTrack = source === TrackSource.Microphone
        ? stream?.getAudioTracks()[0] : stream?.getVideoTracks()[0];
      if (enabled && !newTrack) {
        stopCapture();
        throw new Error('The selected device did not provide a media track');
      }
      // Capture state is native and may change while replaceTrack is awaited.
      const captureEnded = () => newTrack?.readyState === 'ended';
      if (captureEnded()) {
        stopCapture();
        throw new Error('The selected capture stopped before it could be published. Try again.');
      }
      const sender = source === TrackSource.Microphone ? this.micSender
        : source === TrackSource.Camera ? this.cameraSender : this.screenSender;
      try {
        if (sender) {
          await sender.replaceTrack(newTrack ?? null);
        } else if (newTrack && stream) {
          const added = pc.addTrack(newTrack, stream);
          if (source === TrackSource.Microphone) this.micSender = added;
          else if (source === TrackSource.Camera) this.cameraSender = added;
          else this.screenSender = added;
        }
      } catch (err) {
        stopCapture();
        throw err;
      }
      if (!current()) {
        stopCapture();
        // A superseded replacement may have reached the transport while its
        // promise was pending. Restore the committed track (or mute) before
        // the next queued sender change, including when the newer capture fails.
        if (this.pc === pc && sender) await sender.replaceTrack(getTrack());
        return;
      }
      if (captureEnded()) {
        stopCapture();
        if (sender) await sender.replaceTrack(getTrack());
        throw new Error('The selected capture stopped before it could be published. Try again.');
      }
      const oldTrack = getTrack();
      if (oldTrack && oldTrack !== newTrack) { oldTrack.onended = null; oldTrack.stop(); }
      if (stream) this.pendingCaptures[source].delete(stream);
      setTrack(newTrack ?? null);
      this.interruptedMedia.delete(source);
      if (this.localMediaErrorSource === source) this.clearLocalMediaError();
      if (source === TrackSource.Microphone && stream) this.startSpeakingDetection(stream);
      if (newTrack) {
        newTrack.onended = () => {
          if (this.pc !== pc || getTrack() !== newTrack) return;
          if (source === TrackSource.ScreenShare) {
            // The browser's Stop sharing action is an ordinary user stop.
            void this.setScreenShareEnabled(false).catch(() => {});
            return;
          }
          setTrack(null);
          this.interruptedMedia.add(source);
          if (source === TrackSource.Microphone) {
            this.stopSpeakingDetection();
            this.isSpeaking = false;
          }
          this.localMediaErrorSource = source;
          this.localMediaError = source === TrackSource.Camera
            ? 'Camera capture stopped unexpectedly. Turn on the camera to try again.'
            : 'Microphone capture stopped unexpectedly. Unmute the microphone to try again.';
          this.onLocalTrackChanged?.();
          // An old capture may end while a device switch is in progress.
          // Detach only that exact track after queued replacements settle.
          const publisher = source === TrackSource.Microphone ? this.micSender : this.cameraSender;
          const release = this.mediaUpdates[source].then(async () => {
            if (this.pc === pc && publisher?.track === newTrack) await publisher.replaceTrack(null);
          });
          this.mediaUpdates[source] = release.catch(() => {});
        };
      }
      this.onLocalTrackChanged?.();
    });
    // A rejected device switch must not poison later attempts for this source.
    this.mediaUpdates[source] = update.catch(() => {});
    await update;
  }

  async setMicrophoneEnabled(enabled: boolean, opts?: { deviceId?: string }): Promise<void> {
    await this.updateLocalMedia(TrackSource.Microphone, enabled, async () => {
      // Acquire new mic track (respects device selection)
      const constraints: MediaTrackConstraints = {
        echoCancellation: true,
        noiseSuppression: true,
      };
      // `exact`, not a bare value.
      //
      // A bare `deviceId` is an IDEAL constraint: the browser uses that device
      // when it is present and silently substitutes any other when it is not.
      // So a microphone the user chose in settings and later unplugged was
      // quietly replaced by a different one, with nothing said — on the call
      // sites whose own comments say they "respect the selected device".
      // `exact` makes the request mean what those comments claim; the caller
      // decides what to do when it cannot be met.
      if (opts?.deviceId) constraints.deviceId = { exact: opts.deviceId };

      return navigator.mediaDevices.getUserMedia({ audio: constraints });
    });
  }

  async setCameraEnabled(enabled: boolean, opts?: { deviceId?: string }): Promise<void> {
    await this.updateLocalMedia(TrackSource.Camera, enabled, async () => {
      const constraints: MediaTrackConstraints = {
        width: { ideal: 640 },
        height: { ideal: 480 },
        frameRate: { ideal: 24 },
      };
      // `exact` for the same reason as the microphone above — and here it also
      // makes an existing recovery path reachable. `useLocalMedia` catches
      // `NotFoundError`/`OverconstrainedError` to raise "Saved camera not
      // found or disconnected" with a "Use default camera" button; a bare
      // constraint never produces either error, so that prompt could not
      // appear no matter which camera was missing.
      if (opts?.deviceId) constraints.deviceId = { exact: opts.deviceId };

      return navigator.mediaDevices.getUserMedia({ video: constraints });
    });
  }

  async setScreenShareEnabled(enabled: boolean): Promise<void> {
    await this.updateLocalMedia(TrackSource.ScreenShare, enabled, () =>
      navigator.mediaDevices.getDisplayMedia({
        video: true,
        audio: false,
      }),
    );
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
