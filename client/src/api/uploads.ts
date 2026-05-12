/**
 * File uploads: server image (admin), chat attachments (image/video/file),
 * plus the public server-image read endpoint.
 */

import { ApiError, authHeaders, getApiBaseUrl, request } from './core';

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
  const res = await fetch(url, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: formData,
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, body);
  }
  return res.json() as Promise<UploadResponse>;
}

export async function getServerImage(): Promise<{ image_url: string | null }> {
  return request<{ image_url: string | null }>('/api/public/server/image');
}

export async function uploadChatImage(
  pseudonymId: string,
  channelId: string,
  file: File,
): Promise<UploadResponse> {
  const formData = new FormData();
  formData.append('file', file);

  const baseUrl = getApiBaseUrl();
  const url = baseUrl
    ? `${baseUrl}/api/channels/${channelId}/upload`
    : `/api/channels/${channelId}/upload`;
  const res = await fetch(url, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: formData,
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, body);
  }
  return res.json() as Promise<UploadResponse>;
}

/** Generic chat upload (images, videos, files). Same endpoint, server detects type. */
export async function uploadChatFile(
  pseudonymId: string,
  channelId: string,
  file: File,
): Promise<UploadResponse> {
  return uploadChatImage(pseudonymId, channelId, file);
}
