/**
 * File uploads: server image (admin), chat attachments (image/video/file),
 * plus the public server-image read endpoint.
 */

import { authHeaders, fetchAuthenticated, getApiBaseUrl, getCredentialContext, isCredentialContextCurrent, request, throwApiError, StaleCredentialContextError } from './core';
import { beginLiveOperation } from '@/lib/session-ownership';
import { rememberUnpublishedUpload, releaseUnpublishedUpload, removeUnpublishedUpload, type UploadCleanup } from '@/lib/upload-cleanup';

export interface UploadPolicy {
  images_enabled: boolean;
  videos_enabled: boolean;
  files_enabled: boolean;
  max_image_size_mb: number;
  max_video_size_mb: number;
  max_file_size_mb: number;
}

export async function getUploadPolicy(pseudonymId: string, channelId: string, signal?: AbortSignal): Promise<UploadPolicy> {
  const value = await request<UploadPolicy>(`/api/channels/${encodeURIComponent(channelId)}/upload-policy`, { headers: authHeaders(pseudonymId), signal }, 15_000);
  for (const key of ['images_enabled', 'videos_enabled', 'files_enabled'] as const) {
    if (typeof value[key] !== 'boolean') throw new Error('Invalid upload policy');
  }
  for (const key of ['max_image_size_mb', 'max_video_size_mb', 'max_file_size_mb'] as const) {
    if (!Number.isSafeInteger(value[key]) || value[key] < 0) throw new Error('Invalid upload limit');
  }
  return value;
}

/** Read only the signature prefix before sending any body. The server still
 * validates the full file and rechecks policy atomically when committing it. */
export async function checkUploadPolicy(file: File, policy: UploadPolicy): Promise<void> {
  const bytes = new Uint8Array(await file.slice(0, 12).arrayBuffer());
  const ascii = (start: number, length: number) => String.fromCharCode(...bytes.subarray(start, start + length));
  const starts = (...prefix: number[]) => prefix.every((byte, index) => bytes[index] === byte);
  const image = starts(255, 216) || starts(137, 80, 78, 71, 13, 10, 26, 10)
    || ascii(0, 4) === 'GIF8' || ascii(0, 4) === 'RIFF' && ascii(8, 4) === 'WEBP';
  const video = bytes.length >= 12 && ascii(4, 4) === 'ftyp' || starts(0x1a, 0x45, 0xdf, 0xa3);
  const category = image ? 'image' : video ? 'video' : 'file';
  const enabled = image ? policy.images_enabled : video ? policy.videos_enabled : policy.files_enabled;
  const maximum = image ? policy.max_image_size_mb : video ? policy.max_video_size_mb : policy.max_file_size_mb;
  if (!enabled) throw new Error(`${category[0].toUpperCase() + category.slice(1)} uploads are disabled on this server.`);
  if (file.size > Math.min(maximum, 50) * 1024 * 1024) {
    throw new Error(`This ${category} is ${(file.size / 1024 / 1024).toFixed(2)} MB. This server allows ${Math.min(maximum, 50)} MB per ${category}.`);
  }
}

export interface UploadResponse {
  status: string;
  upload_id: string;
  url: string;
  filename?: string;
  content_type?: string;
  category?: 'image' | 'video' | 'file';
  size?: number;
  metadata_stripped_bytes?: number;
}

export async function uploadServerImage(
  pseudonymId: string,
  file: File,
): Promise<UploadResponse> {
  const formData = new FormData();
  formData.append('file', file);

  const baseUrl = getApiBaseUrl();
  const url = baseUrl ? `${baseUrl}/api/admin/server/image` : '/api/admin/server/image';
  const operation = beginLiveOperation(baseUrl, pseudonymId);
  try {
  const res = await fetchAuthenticated(url, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: formData,
    signal: operation.signal,
  });
  if (!res.ok) await throwApiError(res);
  const result = await res.json() as UploadResponse;
  operation.signal.throwIfAborted();
  return result;
  } finally { operation.release(); }
}

export async function getServerImage(): Promise<{ image_url: string | null }> {
  return request<{ image_url: string | null }>('/api/public/server/image');
}

export async function uploadChatImage(
  pseudonymId: string,
  channelId: string,
  file: File,
  signal?: AbortSignal,
): Promise<UploadResponse> {
  const context = getCredentialContext();
  if (context.pseudonymId !== pseudonymId) throw new StaleCredentialContextError('Attachment upload');
  const uploadId = crypto.randomUUID();
  const formData = new FormData();
  formData.append('attachment_id', uploadId);
  formData.append('file', file);

  const baseUrl = context.baseUrl;
  const url = baseUrl
    ? `${baseUrl}/api/channels/${channelId}/upload`
    : `/api/channels/${channelId}/upload`;
  const operation = beginLiveOperation(baseUrl, pseudonymId);
  const joinedSignal = signal ? AbortSignal.any([signal, operation.signal]) : operation.signal;
  let cleanup: UploadCleanup | null = null;
  try {
  joinedSignal.throwIfAborted();
  const policy = await getUploadPolicy(pseudonymId, channelId, joinedSignal);
  await checkUploadPolicy(file, policy);
  joinedSignal.throwIfAborted();
  if (!isCredentialContextCurrent(context)) throw new StaleCredentialContextError('Attachment upload');
  cleanup = rememberUnpublishedUpload(pseudonymId, channelId, uploadId);
  const res = await fetchAuthenticated(url, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: formData,
    signal: joinedSignal,
  });
  if (!res.ok) await throwApiError(res);
  const result = await res.json() as UploadResponse;
  joinedSignal.throwIfAborted();
  if (!isCredentialContextCurrent(context)) throw new StaleCredentialContextError('Attachment upload');
  if (result.status !== 'ok' || result.upload_id !== uploadId) throw new Error('The server did not confirm the prepared attachment ID.');
  releaseUnpublishedUpload(cleanup);
  return result;
  } catch (error) {
    if (cleanup) await removeUnpublishedUpload(cleanup).catch(() => {});
    throw error;
  } finally { operation.release(); }
}

/** Generic chat upload (images, videos, files). Same endpoint, server detects type. */
export async function uploadChatFile(
  pseudonymId: string,
  channelId: string,
  file: File,
  signal?: AbortSignal,
): Promise<UploadResponse> {
  return uploadChatImage(pseudonymId, channelId, file, signal);
}
