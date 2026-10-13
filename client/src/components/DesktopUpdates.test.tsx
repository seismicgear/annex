import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { DesktopUpdates } from './DesktopUpdates';
import * as native from '@/lib/tauri';

vi.mock('@/lib/tauri', () => ({
  getDesktopUpdateStatus: vi.fn(), checkDesktopUpdates: vi.fn(), downloadDesktopUpdate: vi.fn(), installDesktopUpdate: vi.fn(),
  onDesktopUpdateStatus: vi.fn(async () => vi.fn()),
}));
const stopCapture = vi.fn();
vi.mock('@/stores/voice', () => ({ useVoiceStore: { getState: () => ({ forceReset: stopCapture }) } }));

const idle: native.DesktopUpdateStatus = { phase: 'idle', currentVersion: '0.1.0', availableVersion: null, notes: null, downloadedBytes: 0, totalBytes: null, message: null, endpoints: ['https://updates.example/latest.json'] };

describe('DesktopUpdates', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.mocked(native.getDesktopUpdateStatus).mockResolvedValue(idle); });

  it('shows unsupported status without claiming it is up to date or offering unsafe actions', async () => {
    vi.mocked(native.getDesktopUpdateStatus).mockResolvedValue({ ...idle, phase: 'unavailable', endpoints: [], message: 'Updates are disabled for isolated profiles.' });
    render(<DesktopUpdates onClose={vi.fn()} />);
    await screen.findByText('Updates are disabled for isolated profiles.');
    expect(screen.queryByText('This version is up to date.')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Check for updates' })).not.toBeInTheDocument();
    expect(native.checkDesktopUpdates).not.toHaveBeenCalled();
  });

  it('requires a verified native download and a separate explicit install confirmation', async () => {
    const offered = { ...idle, phase: 'available' as const, availableVersion: '0.2.0', notes: 'Release notes' };
    vi.mocked(native.checkDesktopUpdates).mockResolvedValue(offered);
    vi.mocked(native.downloadDesktopUpdate).mockResolvedValue({ ...offered, phase: 'verified' });
    vi.mocked(native.installDesktopUpdate).mockResolvedValue({ ...offered, phase: 'installing' });
    render(<DesktopUpdates onClose={vi.fn()} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Check for updates' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Download update' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Install and restart' }));
    expect(native.installDesktopUpdate).not.toHaveBeenCalled();
    expect(stopCapture).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Confirm install and restart' }));
    await waitFor(() => expect(native.installDesktopUpdate).toHaveBeenCalledTimes(1));
    expect(stopCapture).toHaveBeenCalledTimes(1);
    expect(stopCapture.mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(native.installDesktopUpdate).mock.invocationCallOrder[0]);
  });

  it('keeps signature failure visible, offers retry and never exposes install', async () => {
    const offered = { ...idle, phase: 'available' as const, availableVersion: '0.2.0' };
    vi.mocked(native.getDesktopUpdateStatus).mockResolvedValue(offered);
    vi.mocked(native.downloadDesktopUpdate).mockResolvedValue({ ...offered, phase: 'failed', message: 'The update signature could not be verified. Nothing was installed.' });
    render(<DesktopUpdates onClose={vi.fn()} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Download update' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('signature could not be verified');
    expect(screen.getByRole('button', { name: 'Download update' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Install and restart' })).not.toBeInTheDocument();
    expect(native.installDesktopUpdate).not.toHaveBeenCalled();
  });
});
