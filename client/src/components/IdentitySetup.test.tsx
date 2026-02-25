import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { IdentitySetup } from './IdentitySetup';
import { useIdentityStore } from '@/stores/identity';
import * as tauri from '@/lib/tauri';
import * as startupPrefs from '@/lib/startup-prefs';

// Mock the store hook
vi.mock('@/stores/identity', () => ({
  useIdentityStore: vi.fn(),
}));

// Mock modules
vi.mock('@/lib/tauri', () => ({
  isTauri: vi.fn(),
  clearStartupMode: vi.fn(),
}));

vi.mock('@/lib/startup-prefs', () => ({
  clearWebStartupMode: vi.fn(),
}));

vi.mock('@/components/DeviceLinkDialog', () => ({
  DeviceLinkDialog: () => <div data-testid="device-link-dialog" />,
}));

describe('IdentitySetup', () => {
  const generateLocalKeysMock = vi.fn();
  const importBackupMock = vi.fn();
  const selectIdentityMock = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();

    // Default store mock
    (useIdentityStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      phase: 'uninitialized',
      error: null,
      storedIdentities: [],
      generateLocalKeys: generateLocalKeysMock,
      importBackup: importBackupMock,
      selectIdentity: selectIdentityMock,
    });
  });

  it('clears web startup prefs before creating identity (Web mode)', async () => {
    vi.mocked(tauri.isTauri).mockReturnValue(false);
    render(<IdentitySetup />);

    const createBtn = screen.getByText('Create New Identity');
    fireEvent.click(createBtn);

    await waitFor(() => {
        expect(startupPrefs.clearWebStartupMode).toHaveBeenCalled();
    });
    expect(tauri.clearStartupMode).not.toHaveBeenCalled();
    expect(generateLocalKeysMock).toHaveBeenCalledWith(1);
  });

  it('clears Tauri startup prefs before creating identity (Tauri mode)', async () => {
    vi.mocked(tauri.isTauri).mockReturnValue(true);
    render(<IdentitySetup />);

    const createBtn = screen.getByText('Create New Identity');
    fireEvent.click(createBtn);

    await waitFor(() => {
        expect(tauri.clearStartupMode).toHaveBeenCalled();
    });
    expect(startupPrefs.clearWebStartupMode).not.toHaveBeenCalled();
    expect(generateLocalKeysMock).toHaveBeenCalledWith(1);
  });
});
