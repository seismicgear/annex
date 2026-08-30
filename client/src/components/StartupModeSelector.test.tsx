import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { StartupModeSelector } from './StartupModeSelector';
import { TAURI_DESKTOP_ORIGINS } from '@/lib/tauri';

const getStartupModeMock = vi.fn();
const setApiBaseUrlMock = vi.fn();
const fetchWithTimeoutMock = vi.fn();
const clearStartupModeMock = vi.fn(async () => {});
const clearWebStartupModeMock = vi.fn();
const startEmbeddedServerMock = vi.fn(async () => 'http://127.0.0.1:9999');

vi.mock('@/lib/tauri', async () => {
  const actual = await vi.importActual<typeof import('@/lib/tauri')>('@/lib/tauri');
  return {
    ...actual,
    isTauri: () => true,
    getStartupMode: () => getStartupModeMock(),
    saveStartupMode: vi.fn(async () => {}),
    clearStartupMode: (...args: unknown[]) => clearStartupModeMock(...args),
    startEmbeddedServer: (...args: unknown[]) => startEmbeddedServerMock(...args),
    acquirePublicEndpoint: vi.fn(async () => 'https://host-abc123.router.annex.net'),
    getWebRtcConfig: vi.fn(async () => ({ configured: false, url: '', api_key: '', has_api_secret: false, token_ttl_seconds: 3600 })),
    startLocalWebRtc: vi.fn(async () => ({ url: 'ws://127.0.0.1:7880' })),
    exportIdentityJson: vi.fn(async () => null),
    getPlatformMediaStatus: vi.fn(async () => ({ screen_share_available: true, camera_mic_available: true, warnings: [], display_server: 'test' })),
    checkWebRtcReachable: vi.fn(async () => ({ reachable: true })),
  };
});

vi.mock('@/lib/api', () => ({
  setApiBaseUrl: (...args: unknown[]) => setApiBaseUrlMock(...args),
  fetchWithTimeout: (...args: unknown[]) => fetchWithTimeoutMock(...args),
}));

vi.mock('@/lib/startup-prefs', () => ({ clearWebStartupMode: (...args: unknown[]) => clearWebStartupModeMock(...args) }));

const mockBeginRemoteRegistration = vi.fn(async () => ({ id: 'srv-1', baseUrl: '', slug: 'test', label: 'Test' }));
vi.mock('@/stores/servers', () => ({
  useServersStore: {
    getState: () => ({
      findServerByBaseUrl: () => null,
      beginRemoteRegistration: (...args: unknown[]) => mockBeginRemoteRegistration(...args),
    }),
  },
}));

vi.mock('@/stores/identity', () => ({
  useIdentityStore: {
    getState: () => ({
      selectIdentity: vi.fn(async () => {}),
    }),
  },
}));

const mockSetVoiceSessionDisabled = vi.fn();
vi.mock('@/stores/voice', () => ({
  useVoiceStore: {
    getState: () => ({
      setVoiceSessionDisabled: mockSetVoiceSessionDisabled,
    }),
  },
}));

describe('StartupModeSelector', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getStartupModeMock.mockResolvedValue({
      startup_mode: { mode: 'client', server_url: 'https://unreachable.invalid' },
    });
    fetchWithTimeoutMock.mockResolvedValue({ ok: false, status: 503 } as Response);
    global.fetch = vi.fn(async () => ({ ok: false, status: 503 } as Response));
  });

  it('keeps choose phase and pre-fills unreachable client URL from startup prefs', async () => {
    const onReady = vi.fn();
    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => {
      expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
    });

    expect(screen.getByDisplayValue('https://unreachable.invalid')).toBeInTheDocument();
    expect(screen.queryByText('Connecting to server...')).not.toBeInTheDocument();
    expect(onReady).not.toHaveBeenCalled();
    expect(setApiBaseUrlMock).not.toHaveBeenCalled();
  });

  it('shows all supported desktop origins in CORS error', async () => {
    getStartupModeMock.mockResolvedValue(null);
    // Simulate a CORS-like fetch failure
    fetchWithTimeoutMock.mockRejectedValue(new TypeError('Failed to fetch'));

    const onReady = vi.fn();
    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText('annex.example.com')).toBeInTheDocument();
    });

    // Fill in a server URL and submit
    const input = screen.getByPlaceholderText('annex.example.com');
    fireEvent.change(input, { target: { value: 'https://my-server.example.com' } });
    fireEvent.submit(input.closest('form')!);

    await waitFor(() => {
      const errorEl = screen.getByText(/desktop origins/i);
      expect(errorEl).toBeInTheDocument();
    });

    // Verify all three origins are mentioned
    const errorText = screen.getByText(/desktop origins/i).textContent ?? '';
    for (const origin of TAURI_DESKTOP_ORIGINS) {
      expect(errorText).toContain(origin);
    }
  });

  it('shows timeout message for slow server probe', async () => {
    getStartupModeMock.mockResolvedValue(null);
    fetchWithTimeoutMock.mockRejectedValue(new Error('Request timed out after 15000ms'));

    const onReady = vi.fn();
    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText('annex.example.com')).toBeInTheDocument();
    });

    const input = screen.getByPlaceholderText('annex.example.com');
    fireEvent.change(input, { target: { value: 'https://slow.example.com' } });
    fireEvent.submit(input.closest('form')!);

    await waitFor(() => {
      expect(screen.getByText(/did not respond in time/i)).toBeInTheDocument();
    });
  });

  it('clears voiceSessionDisabled when connecting to a remote server', async () => {
    getStartupModeMock.mockResolvedValue(null);
    fetchWithTimeoutMock.mockResolvedValue({ ok: true, status: 200 } as Response);

    const onReady = vi.fn();
    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText('annex.example.com')).toBeInTheDocument();
    });

    const input = screen.getByPlaceholderText('annex.example.com');
    fireEvent.change(input, { target: { value: 'https://good-server.example.com' } });
    fireEvent.submit(input.closest('form')!);

    await waitFor(() => {
      expect(mockSetVoiceSessionDisabled).toHaveBeenCalledWith(false);
    });

    expect(onReady).toHaveBeenCalled();
  });

  it('clears voiceSessionDisabled on remote server connect (integration)', async () => {
    // Verify that when connecting to a remote server succeeds,
    // setVoiceSessionDisabled(false) is called before onReady
    getStartupModeMock.mockResolvedValue(null);
    fetchWithTimeoutMock.mockResolvedValue({ ok: true, status: 200 } as Response);

    const onReady = vi.fn();
    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText('annex.example.com')).toBeInTheDocument();
    });

    const input = screen.getByPlaceholderText('annex.example.com');
    fireEvent.change(input, { target: { value: 'https://remote.example.com' } });
    fireEvent.submit(input.closest('form')!);

    await waitFor(() => {
      // setVoiceSessionDisabled(false) should be called before onReady
      const calls = mockSetVoiceSessionDisabled.mock.calls;
      const falseCalls = calls.filter((c: unknown[]) => c[0] === false);
      expect(falseCalls.length).toBeGreaterThanOrEqual(1);
    });
  });


  it('says what failed and offers a way out, on the screen a failed start lands on', async () => {
    // This is the whole app for a user whose chosen mode will not start, and
    // it used to be `<h1>Annex</h1>`, a bare exception string, and a button
    // labelled "Try Again" that actually returns to the chooser. The sibling
    // screen in `StartupGate` labels its error and styles its button; this
    // one is reached more often, because it covers every way a local server
    // can fail to come up.
    getStartupModeMock
      .mockResolvedValueOnce({ startup_mode: { mode: 'host' } })
      .mockResolvedValue(null);
    startEmbeddedServerMock.mockRejectedValueOnce(new Error('port 3000 in use'));

    render(<StartupModeSelector onReady={vi.fn()} />);

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Startup failed');
    expect(alert.textContent).toContain('port 3000 in use');

    // Named for what it does. It clears the saved mode and returns to the
    // chooser — it does not retry anything.
    const back = screen.getByRole('button', { name: /back to setup options/i });
    expect(back).toHaveClass('primary-btn');

    fireEvent.click(back);
    await waitFor(() => {
      expect(
        screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.'),
      ).toBeInTheDocument();
    });
  });

  it('clears persisted host startup mode after auto-host failure and does not auto-retry on next render', async () => {
    getStartupModeMock
      .mockResolvedValueOnce({ startup_mode: { mode: 'host' } })
      .mockResolvedValue(null);
    startEmbeddedServerMock.mockRejectedValueOnce(new Error('boot failed'));

    const onReady = vi.fn();
    const { unmount } = render(<StartupModeSelector onReady={onReady} />);

    // The message is now composed as "Startup failed: <reason>", so match
    // the alert's text rather than an exact standalone node.
    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toContain('boot failed');
    });

    await waitFor(() => {
      expect(clearStartupModeMock).toHaveBeenCalledTimes(1);
    });
    expect(clearWebStartupModeMock).not.toHaveBeenCalled();

    unmount();
    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => {
      expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
    });

    expect(startEmbeddedServerMock).toHaveBeenCalledTimes(1);
  });
});
