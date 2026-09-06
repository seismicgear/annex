/**
 * Web/Docker startup path.
 *
 * Lives in its own file because `StartupModeSelector.test.tsx` mocks
 * `isTauri` to `true` for the whole module, and these cases need the
 * opposite. Splitting the file is cheaper and clearer than making that
 * mock mutable.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { StartupModeSelector } from './StartupModeSelector';

const loadWebStartupModeMock = vi.fn();
const saveWebStartupModeMock = vi.fn();
const setApiBaseUrlMock = vi.fn();

vi.mock('@/lib/tauri', async () => {
  const actual = await vi.importActual<typeof import('@/lib/tauri')>('@/lib/tauri');
  return {
    ...actual,
    isTauri: () => false,
    getStartupMode: vi.fn(async () => null),
    saveStartupMode: vi.fn(async () => {}),
    clearStartupMode: vi.fn(async () => {}),
    getPlatformMediaStatus: vi.fn(async () => ({
      screen_share_available: true,
      camera_mic_available: true,
      warnings: [],
      display_server: 'test',
    })),
  };
});

vi.mock('@/lib/api', () => ({
  setApiBaseUrl: (...args: unknown[]) => setApiBaseUrlMock(...args),
  fetchWithTimeout: vi.fn(),
}));

vi.mock('@/lib/startup-prefs', () => ({
  loadWebStartupMode: () => loadWebStartupModeMock(),
  saveWebStartupMode: (...args: unknown[]) => saveWebStartupModeMock(...args),
  clearWebStartupMode: vi.fn(),
}));

vi.mock('@/stores/voice', () => ({
  useVoiceStore: {
    getState: () => ({ setVoiceSessionDisabled: vi.fn() }),
  },
}));

vi.mock('@/stores/servers', () => ({
  useServersStore: {
    getState: () => ({
      findServerByBaseUrl: () => null,
      beginRemoteRegistration: vi.fn(),
    }),
  },
}));

describe('StartupModeSelector — web/Docker', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('auto-resumes "use this server" instead of re-asking on every load', async () => {
    // A returning user who already chose this server was previously shown the
    // chooser again on every page load, and answering it drove a redundant
    // re-registration against a server they had already joined.
    loadWebStartupModeMock.mockReturnValue({ mode: 'local' });
    const onReady = vi.fn();

    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() => expect(onReady).toHaveBeenCalled());
    expect(setApiBaseUrlMock).toHaveBeenCalledWith('');
    // Auto-resume must not rewrite the preference it just read.
    expect(saveWebStartupModeMock).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
  });

  it('shows the chooser on a first visit', async () => {
    loadWebStartupModeMock.mockReturnValue(null);
    const onReady = vi.fn();

    render(<StartupModeSelector onReady={onReady} />);

    expect(await screen.findByRole('button', { name: 'Continue' })).toBeInTheDocument();
    expect(onReady).not.toHaveBeenCalled();
  });

  it('pre-fills a saved remote URL without auto-connecting to it', async () => {
    // Remote deliberately stays manual: auto-resuming an unreachable host
    // would flash "Connecting..." and then strand the user on an error
    // screen with an empty URL field.
    loadWebStartupModeMock.mockReturnValue({
      mode: 'remote',
      server_url: 'https://annex.example',
    });
    const onReady = vi.fn();

    render(<StartupModeSelector onReady={onReady} />);

    await waitFor(() =>
      expect(screen.getByDisplayValue('https://annex.example')).toBeInTheDocument(),
    );
    expect(onReady).not.toHaveBeenCalled();
  });
});
