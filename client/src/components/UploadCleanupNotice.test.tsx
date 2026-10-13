import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

beforeEach(() => { vi.resetModules(); localStorage.clear(); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
it('keeps a failed removal visible after navigation and removes its durable record only after a confirmed retry', async () => {
  let attempts = 0;
  vi.stubGlobal('fetch', vi.fn(async (_url: string, init: RequestInit) => init.method === 'DELETE'
    ? (++attempts === 1 ? json({ error: 'Disk cleanup pending' }, 409) : json({ status: 'ok' }))
    : json({ token: 'ordinary', expiresInSecs: 300 })));
  const core = await import('@/api/core'); core.setApiBaseUrl('https://annex.example'); core.setSessionToken('token', 'alice');
  const pending = await import('@/lib/upload-cleanup');
  const entry = pending.rememberUnpublishedUpload('alice', 'room', '11111111-1111-4111-8111-111111111111');
  await expect(pending.removeUnpublishedUpload(entry)).rejects.toThrow('Disk cleanup pending');
  const { UploadCleanupNotice } = await import('./UploadCleanupNotice');
  const view = render(<UploadCleanupNotice />);
  expect(screen.getByRole('alert')).toHaveTextContent('Attachment removal is not confirmed. Disk cleanup pending');
  view.unmount(); render(<UploadCleanupNotice />);
  expect(screen.getByRole('button', { name: 'Retry attachment removal' })).toBeEnabled();
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Retry attachment removal' })); });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(pending.getUploadCleanups()).toEqual([]);
  expect(localStorage.getItem('annex.unpublished-upload-cleanup.v1')).toBe('[]');
});
it('restores an interrupted in-flight upload as pending without storing keys or disclosing it on another server', async () => {
  localStorage.setItem('annex.unpublished-upload-cleanup.v1', JSON.stringify([{ baseUrl: 'https://original.example', pseudonym: 'alice', channelId: 'room',
    uploadId: '11111111-1111-4111-8111-111111111111', pending: false, error: null }]));
  vi.stubGlobal('fetch', vi.fn(async () => json({ token: 'ordinary', expiresInSecs: 300 })));
  const core = await import('@/api/core'); core.setApiBaseUrl('https://different.example'); core.setSessionToken('token', 'alice');
  const pending = await import('@/lib/upload-cleanup');
  expect(pending.getUploadCleanups()[0].pending).toBe(true);
  const { UploadCleanupNotice } = await import('./UploadCleanupNotice'); const view = render(<UploadCleanupNotice />);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  core.setApiBaseUrl('https://original.example'); core.setSessionToken('original-token', 'alice'); view.rerender(<UploadCleanupNotice />);
  expect(screen.getByRole('alert')).toHaveTextContent('An interrupted attachment upload needs removal.');
});
