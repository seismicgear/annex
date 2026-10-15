/** Channel encryption toggle API. Key distribution uses the certified v2 protocol. */
import { getE2eControls } from '@/lib/e2e-store';
import { E2eTrustRequiredError } from '@/lib/e2e-manifest-session';

/** Whether a channel is end-to-end encrypted. */
export async function getChannelE2e(pseudonymId: string, channelId: string): Promise<boolean> {
  const { state, lineage } = await getE2eControls(pseudonymId).inspectChannel(channelId);
  if (lineage.final.head && !lineage.trusted) throw new E2eTrustRequiredError({
    authorityIdentity: lineage.final.authorityIdentity, channelId, genesisHash: lineage.final.head.genesisHash });
  return state.e2e_enabled;
}

/** Toggle E2E on a channel (requires moderation capability server-side). */
export async function setChannelE2e(
  pseudonymId: string,
  channelId: string,
  enabled: boolean,
): Promise<void> {
  await getE2eControls(pseudonymId).setEncryption(channelId, enabled);
}
