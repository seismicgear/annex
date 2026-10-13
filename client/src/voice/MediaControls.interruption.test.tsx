import { useEffect, useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { WebRtcSession } from '@/lib/webrtc';
import { useVoiceStore } from '@/stores/voice';
import { MediaControls } from './MediaControls';

vi.mock('@/lib/tauri', () => ({ isTauri: () => false, invoke: vi.fn() }));

beforeEach(() => useVoiceStore.setState({ micMuted: true, cameraDeviceId: null, inputDeviceId: null }));

it('shows a post-capture native interruption with a usable camera retry and dismiss action', () => {
  const session = new WebRtcSession([], 'room', 'member', {
    sendOffer: vi.fn(), sendAnswer: vi.fn(), sendIceCandidate: vi.fn(),
  });
  session.localMediaError = 'Camera capture stopped unexpectedly. Turn on the camera to try again.';
  function Controls() {
    const [, renderTrackState] = useState(0);
    useEffect(() => {
      session.onLocalTrackChanged = () => renderTrackState(value => value + 1);
      return () => { session.onLocalTrackChanged = undefined; };
    }, []);
    return <MediaControls localParticipant={session} isMicrophoneEnabled={false}
      isCameraEnabled={false} isScreenShareEnabled={false} onLeave={vi.fn()}
      mediaStatus={null} platformWarnings={[]} />;
  }
  render(<Controls />);
  expect(screen.getByRole('alert')).toHaveTextContent('Camera capture stopped unexpectedly');
  expect(screen.getByTitle('Turn on camera')).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: /^Dismiss$/ }));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(session.localMediaError).toBeNull();
});
