import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { StartupModeSelector } from './StartupModeSelector';

const getStartupModeMock = vi.fn();
const setApiBaseUrlMock = vi.fn();

vi.mock('@/lib/tauri', () => ({
  isTauri: () => true,
  getStartupMode: () => getStartupModeMock(),
  saveStartupMode: vi.fn(async () => {}),
  clearStartupMode: vi.fn(async () => {}),
  startEmbeddedServer: vi.fn(async () => 'http://127.0.0.1:9999'),
  startTunnel: vi.fn(async () => 'https://tunnel.example.com'),
  getLiveKitConfig: vi.fn(async () => ({ configured: false, url: '', api_key: '', has_api_secret: false, token_ttl_seconds: 3600 })),
  startLocalLiveKit: vi.fn(async () => ({ url: 'ws://127.0.0.1:7880' })),
  exportIdentityJson: vi.fn(async () => null),
  getPlatformMediaStatus: vi.fn(async () => ({ screen_share_available: true, camera_mic_available: true, warnings: [], display_server: 'test' })),
}));

vi.mock('@/lib/api', () => ({
  setApiBaseUrl: (...args: unknown[]) => setApiBaseUrlMock(...args),
}));

vi.mock('@/lib/startup-prefs', () => ({ clearWebStartupMode: vi.fn() }));

describe('StartupModeSelector', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getStartupModeMock.mockResolvedValue({
      startup_mode: { mode: 'client', server_url: 'https://unreachable.invalid' },
    });
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
    expect(global.fetch).not.toHaveBeenCalled();
    expect(onReady).not.toHaveBeenCalled();
    expect(setApiBaseUrlMock).not.toHaveBeenCalled();
  });
});
