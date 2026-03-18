import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { StartupModeSelector } from './StartupModeSelector';
import { TAURI_DESKTOP_ORIGINS } from '@/lib/tauri';

const getStartupModeMock = vi.fn();
const setApiBaseUrlMock = vi.fn();
const fetchWithTimeoutMock = vi.fn();

vi.mock('@/lib/tauri', async () => {
  const actual = await vi.importActual<typeof import('@/lib/tauri')>('@/lib/tauri');
  return {
    ...actual,
    isTauri: () => true,
    getStartupMode: () => getStartupModeMock(),
    saveStartupMode: vi.fn(async () => {}),
    clearStartupMode: vi.fn(async () => {}),
    startEmbeddedServer: vi.fn(async () => 'http://127.0.0.1:9999'),
    acquirePublicEndpoint: vi.fn(async () => 'https://host-abc123.router.annex.net'),
    getLiveKitConfig: vi.fn(async () => ({ configured: false, url: '', api_key: '', has_api_secret: false, token_ttl_seconds: 3600 })),
    startLocalLiveKit: vi.fn(async () => ({ url: 'ws://127.0.0.1:7880' })),
    exportIdentityJson: vi.fn(async () => null),
    getPlatformMediaStatus: vi.fn(async () => ({ screen_share_available: true, camera_mic_available: true, warnings: [], display_server: 'test' })),
    checkLiveKitReachable: vi.fn(async () => ({ reachable: true })),
  };
});

vi.mock('@/lib/api', () => ({
  setApiBaseUrl: (...args: unknown[]) => setApiBaseUrlMock(...args),
  fetchWithTimeout: (...args: unknown[]) => fetchWithTimeoutMock(...args),
}));

vi.mock('@/lib/startup-prefs', () => ({ clearWebStartupMode: vi.fn() }));

vi.mock('@/stores/servers', () => ({
  useServersStore: {
    getState: () => ({
      findServerByBaseUrl: () => null,
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
});
