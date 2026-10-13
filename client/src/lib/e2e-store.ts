/** Production device/epoch manager, scoped to one credential context. */
import { getCredentialContext } from '@/api/core';
import { boundE2eApiV2, type E2eControls } from '@/api/e2e-v2';
import { E2eChannelManagerV2 } from './e2e-manager-v2';
import { closeE2eKeyringV2, indexedDbKeyringV2 } from './e2e-keyring-v2';
import { cancelMembershipVerifications } from './membership-verification';
import { clearVerifiedChannels } from './verified-channel';

const managers = new Map<string, E2eChannelManagerV2>();
const controls = new Map<string, E2eControls>();

export function getE2eControls(pseudonymId: string): E2eControls {
  getE2eChannelManager(pseudonymId);
  const context = getCredentialContext();
  return controls.get(JSON.stringify([context.serial, context.baseUrl, pseudonymId]))!;
}

export function getE2eChannelManager(pseudonymId: string): E2eChannelManagerV2 {
  const context = getCredentialContext();
  const namespace = JSON.stringify([context.serial, context.baseUrl, pseudonymId]);
  let manager = managers.get(namespace);
  if (!manager) {
    const api = boundE2eApiV2(pseudonymId);
    manager = new E2eChannelManagerV2(pseudonymId, api, indexedDbKeyringV2());
    managers.set(namespace, manager);
    controls.set(namespace, api);
  }
  return manager;
}

export function clearE2eManagers(): void {
  cancelMembershipVerifications();
  clearVerifiedChannels();
  for (const manager of managers.values()) manager.forget();
  managers.clear();
  controls.clear();
}

/** Close the upgraded database before any explicit fresh-install deletion. */
export async function resetE2eDbHandle(): Promise<void> {
  clearE2eManagers();
  await closeE2eKeyringV2();
}
