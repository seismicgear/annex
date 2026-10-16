import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { transportFetch } from '@/api/hosted-fetch';
import { HostedDownload, HostedImage } from './HostedAsset';
vi.mock('@/api/hosted-fetch', () => ({ transportFetch: vi.fn() }));
const source = 'https://www.monolithannex.com/host/' + 'a'.repeat(64) + '/uploads/file.png';
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.clearAllMocks(); });
describe('encrypted hosted assets', () => {
  it('shows a usable retry after an encrypted image request fails', async () => {
    vi.mocked(transportFetch).mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce(new Response('image'));
    vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:verified-image');
    const revoke = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
    const view = render(<HostedImage src={source} alt="Shared photo" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Retry image' }));
    await waitFor(() => expect(screen.getByAltText('Shared photo').getAttribute('src')).toBe('blob:verified-image'));
    expect(transportFetch).toHaveBeenCalledTimes(2); view.unmount(); expect(revoke).toHaveBeenCalledWith('blob:verified-image');
  });
  it('downloads only after a click and retries failures without navigating to plaintext HTTP', async () => {
    vi.mocked(transportFetch).mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce(new Response('private file'));
    vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:verified-download');
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    render(<HostedDownload href={source} download="photo.png">Save file</HostedDownload>);
    expect(transportFetch).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('link', { name: 'Save file' }));
    const retry = await screen.findByRole('link', { name: /Download failed. Click to retry/ });
    expect(retry.getAttribute('href')).toBe('#'); fireEvent.click(retry);
    await waitFor(() => expect(click).toHaveBeenCalledOnce());
    expect(transportFetch).toHaveBeenCalledTimes(2);
    const anchor = click.mock.instances[0] as HTMLAnchorElement;
    expect(anchor.getAttribute('href')).toBe('blob:verified-download');
    expect(anchor.getAttribute('download')).toBe('photo.png');
  });
});
