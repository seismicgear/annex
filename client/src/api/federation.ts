/**
 * Federation: server discovery (local + remote) and peer listings.
 */

import type { FederationPeer, ServerSummary } from '@/types';
import { request, requestRemote } from './core';

export async function getServerSummary(): Promise<ServerSummary> {
  return request<ServerSummary>('/api/public/server/summary');
}

export async function getFederationPeers(): Promise<{ peers: FederationPeer[] }> {
  return request<{ peers: FederationPeer[] }>('/api/public/federation/peers');
}

export async function getRemoteServerSummary(
  baseUrl: string,
): Promise<ServerSummary> {
  return requestRemote<ServerSummary>(baseUrl, '/api/public/server/summary');
}

export async function getRemoteFederationPeers(
  baseUrl: string,
): Promise<{ peers: FederationPeer[] }> {
  return requestRemote<{ peers: FederationPeer[] }>(baseUrl, '/api/public/federation/peers');
}
