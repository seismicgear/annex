import { expect, test, type Locator } from '@playwright/test';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { baseURL, newActor, publicIdentities, read, reload, row, select, type Actor } from './operator-helpers';
import type { ServerPolicy } from '../../src/types';

type Metadata = { capability_contract: { offered_capabilities: string[] }; [key: string]: unknown };
type FixtureReply = { ok: boolean; error?: string; pseudonym: string; metadata: Metadata; backup?: string };
type GrantSnapshot = { owner_pseudonym: string; metadata_json: string | null; grantees: string[] };
type Profile = { pseudonym_id: string; node_type: string; active: boolean; metadata_json?: string | null; visibility: string };

class GenuineAgent {
  private child: ChildProcessWithoutNullStreams;
  private buffer = '';
  private waiting: { resolve: (value: FixtureReply) => void; reject: (error: Error) => void; timer: NodeJS.Timeout } | null = null;
  constructor() {
    this.child = spawn(process.execPath, [fileURLToPath(new URL('../../../scripts/live/metadata-agent.mjs', import.meta.url))], {
      windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'],
    });
    // Both pipes may contain secret protocol material. They are never printed,
    // attached to reports, or used in assertion messages.
    this.child.stderr.resume();
    this.child.stdout.on('data', (chunk: Buffer) => {
      this.buffer += chunk.toString('utf8');
      if (this.buffer.length > 1_048_576) { this.fail('Agent helper exceeded its private response bound'); return; }
      let end: number;
      while ((end = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0, end); this.buffer = this.buffer.slice(end + 1);
        if (!this.waiting) continue;
        try {
          const result: FixtureReply = JSON.parse(line);
          if (!result.ok) { this.fail(result.error ?? 'Genuine agent operation failed'); continue; }
          const waiting = this.waiting; this.waiting = null; clearTimeout(waiting.timer); waiting.resolve(result);
        } catch { this.fail('Agent helper returned an invalid private response'); }
      }
    });
    this.child.on('error', () => this.fail('Could not launch the genuine agent helper'));
    this.child.on('exit', () => this.fail('Genuine agent helper stopped before completing the operation'));
  }
  private fail(message: string) {
    if (this.waiting) { const waiting = this.waiting; this.waiting = null; clearTimeout(waiting.timer); waiting.reject(new Error(message)); }
  }
  async command(command: Record<string, unknown>): Promise<FixtureReply> {
    if (this.waiting) throw new Error('Agent helper operation already running');
    return new Promise((resolve, reject) => {
      this.waiting = { resolve, reject, timer: setTimeout(() => this.fail('Genuine agent operation timed out'), 150_000) };
      this.child.stdin.write(JSON.stringify(command) + '\n');
    });
  }
  close() { this.fail('Agent helper closed'); this.buffer = ''; this.child.kill(); }
}

async function sharing(who: Actor) {
  await who.page.getByRole('button', { name: 'Identity', exact: true }).click();
  const dialog = who.page.getByRole('dialog');
  const section = dialog.getByRole('region', { name: 'Profile details sharing', exact: true });
  await section.getByRole('button', { name: 'Manage profile sharing', exact: true }).click();
  await expect(section.getByText('Loading profile sharing…', { exact: true })).toHaveCount(0);
  return { dialog, section };
}
function recipient(section: Locator, pseudonym: string) {
  return section.locator('.metadata-sharing-person').filter({ has: section.page().locator(`span[title="${pseudonym}"]`) });
}
async function change(section: Locator, pseudonym: string, grant: boolean, confirm: boolean) {
  await recipient(section, pseudonym).getByRole('button', { name: grant ? 'Grant profile sharing' : 'Revoke profile sharing', exact: true }).click();
  const prompt = section.getByRole('region', { name: 'Confirm profile sharing change', exact: true });
  await expect(prompt).toContainText(pseudonym);
  if (grant) await expect(prompt).toContainText('future updates');
  await prompt.getByRole('button', { name: confirm ? 'Confirm sharing change' : 'Cancel sharing change', exact: true }).click();
  await expect(prompt).toHaveCount(0);
  if (confirm) await expect(section.getByText(`Profile details sharing ${grant ? 'granted' : 'revoked'}.`, { exact: true })).toBeVisible();
}
const profile = async (who: Actor, owner: string) => {
  const value = await read<Profile>(who, `/api/graph/profile/${owner}`);
  // GraphProfile intentionally omits its optional metadata field when private.
  return { ...value, metadata_json: value.metadata_json ?? null };
};
const grants = (who: Actor) => read<GrantSnapshot>(who, '/api/graph/metadata-grants');

test('actual owner-controlled profile details: private default, explicit current/future sharing, topology and revocation', async ({ browser }, info) => {
  const actors: Actor[] = []; const helper = new GenuineAgent();
  let backup: string | undefined;
  const markers = [`PROFILE_METADATA_${Date.now()}_FIRST`, `PROFILE_METADATA_${Date.now()}_UPDATED`];
  const receipts: string[] = [];
  try {
    const founder = await newActor(browser); actors.push(founder);
    const viewer = await newActor(browser); actors.push(viewer);
    const distant = await newActor(browser); actors.push(distant);
    const agent = await newActor(browser, false); actors.push(agent);
    const viewerId = (await publicIdentities(viewer))[0].pseudonymId!;
    const distantId = (await publicIdentities(distant))[0].pseudonymId!;
    const policy = await read<ServerPolicy>(founder, '/api/admin/policy');
    const config = await read<{ slug: string }>(founder, '/api/admin/server');
    let created!: FixtureReply;
    await test.step('genuine role-2 proof and handshake import through the normal backup UI; actual shared channel', async () => {
      created = await helper.command({ command: 'create', url: baseURL, serverSlug: config.slug, policy, marker: markers[0] });
      backup = created.backup; delete created.backup;
      if (!backup) throw new Error('Genuine agent helper did not supply an importable identity');
      await agent.page.goto('/');
      await agent.page.getByLabel('Choose an Annex backup file', { exact: true }).setInputFiles({ name: 'agent-identity.json', mimeType: 'application/json', buffer: Buffer.from(backup) });
      backup = undefined;
      await agent.page.locator('.import-actions').getByRole('button', { name: 'Import', exact: true }).click();
      await agent.page.getByRole('button', { name: 'Continue', exact: true }).click();
      await expect(agent.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
      expect((await publicIdentities(agent))[0].pseudonymId).toBe(created.pseudonym);
      await select(founder, 'General'); await select(viewer, 'General'); await select(agent, 'General');
      const snapshot = await grants(agent);
      expect(JSON.parse(snapshot.metadata_json!)).toEqual(created.metadata);
      expect(snapshot.grantees).toEqual([]);
      receipts.push('actual-role2-registry-proof-vrp-and-normal-import');
    });
    await test.step('human empty details are honest; connected agent profile remains private without a grant', async () => {
      const human = await sharing(viewer);
      await expect(human.section.getByText('No additional profile details are stored for this identity.', { exact: true })).toBeVisible();
      await change(human.section, created.pseudonym, true, false);
      expect((await grants(viewer)).grantees).toEqual([]);
      await human.dialog.getByRole('button', { name: 'Close', exact: true }).click();
      const visible = await profile(viewer, created.pseudonym);
      expect(visible.metadata_json).toBeNull();
      expect(visible.visibility).not.toBe('None');
      expect(visible.active).toBe(true);
      receipts.push('connected-profile-private-by-default-and-null-human-preview');
    });
    await test.step('cancel makes no grant; explicit confirmation reveals exact actual metadata only to the selected viewer', async () => {
      const owner = await sharing(agent);
      await expect(owner.section.getByLabel('Current profile details', { exact: true })).toContainText(markers[0]);
      await change(owner.section, viewerId, true, false);
      expect((await grants(agent)).grantees).toEqual([]);
      expect((await profile(viewer, created.pseudonym)).metadata_json).toBeNull();
      await change(owner.section, viewerId, true, true);
      expect(JSON.parse((await profile(viewer, created.pseudonym)).metadata_json!)).toEqual(created.metadata);
      expect((await profile(founder, created.pseudonym)).metadata_json).toBeNull();
      await owner.dialog.getByRole('button', { name: 'Close', exact: true }).click();
      await reload(agent);
      const reopened = await sharing(agent);
      await expect(recipient(reopened.section, viewerId).getByRole('button', { name: 'Revoke profile sharing', exact: true })).toBeVisible();
      await reopened.dialog.getByRole('button', { name: 'Close', exact: true }).click();
      receipts.push('confirmed-grant-exact-agent-metadata-reader-only-and-reload');
    });
    await test.step('a grant does not create graph visibility; later actual channel membership permits the existing grant', async () => {
      // Establish absence through actual join/leave, independent of whether a
      // future registration flow also admits new people to General.
      await select(distant, 'General');
      const entry = row(distant, 'General');
      const departed = distant.page.waitForResponse(response => response.request().method() === 'POST'
        && new URL(response.url()).pathname.endsWith('/leave'));
      await entry.locator('.leave-btn').click();
      expect((await departed).status()).toBe(200);
      await expect(entry.locator('.join-btn')).toBeVisible();
      const before = await profile(distant, created.pseudonym);
      expect(before.visibility).toBe('None'); expect(before.metadata_json).toBeNull();
      const owner = await sharing(agent);
      await change(owner.section, distantId, true, true);
      const stillDistant = await profile(distant, created.pseudonym);
      expect(stillDistant.visibility).toBe('None'); expect(stillDistant.metadata_json).toBeNull();
      await select(distant, 'General');
      const joined = await profile(distant, created.pseudonym);
      expect(joined.visibility).not.toBe('None');
      expect(JSON.parse(joined.metadata_json!)).toEqual(created.metadata);
      await owner.dialog.getByRole('button', { name: 'Close', exact: true }).click();
      receipts.push('grant-does-not-widen-topology-actual-join-resolves-visibility');
    });
    await test.step('the existing grant covers actual later handshake metadata updates without a second grant', async () => {
      const updated = await helper.command({ command: 'update', marker: markers[1] });
      expect(updated.pseudonym).toBe(created.pseudonym);
      expect(updated.metadata.capability_contract.offered_capabilities).toContain(markers[1]);
      expect(updated.metadata.capability_contract.offered_capabilities).not.toContain(markers[0]);
      expect(JSON.parse((await profile(viewer, created.pseudonym)).metadata_json!)).toEqual(updated.metadata);
      expect(JSON.parse((await profile(distant, created.pseudonym)).metadata_json!)).toEqual(updated.metadata);
      expect((await grants(agent)).grantees.sort()).toEqual([viewerId, distantId].sort());
      const owner = await sharing(agent);
      await expect(owner.section.getByLabel('Current profile details', { exact: true })).toContainText(markers[1]);
      await owner.dialog.getByRole('button', { name: 'Close', exact: true }).click();
      receipts.push('genuine-vrp-update-covered-by-existing-explicit-grant');
    });
    await test.step('revoke cancel preserves sharing; confirm removes only that reader’s metadata and survives reload', async () => {
      const before = await profile(viewer, created.pseudonym);
      const owner = await sharing(agent);
      await change(owner.section, viewerId, false, false);
      expect((await profile(viewer, created.pseudonym)).metadata_json).toBe(before.metadata_json);
      await change(owner.section, viewerId, false, true);
      const after = await profile(viewer, created.pseudonym);
      expect(after.metadata_json).toBeNull();
      expect(after.visibility).toBe(before.visibility); expect(after.node_type).toBe(before.node_type); expect(after.active).toBe(before.active);
      expect((await profile(distant, created.pseudonym)).metadata_json).toBe(before.metadata_json);
      await change(owner.section, distantId, false, true);
      expect((await grants(agent)).grantees).toEqual([]);
      await owner.dialog.getByRole('button', { name: 'Close', exact: true }).click();
      await reload(agent); await reload(viewer);
      expect((await profile(viewer, created.pseudonym)).metadata_json).toBeNull();
      receipts.push('cancel-confirm-revoke-per-reader-and-reload');
    });
    for (const actor of actors) expect(actor.errors).toEqual([]);
  } finally {
    backup = undefined; helper.close();
    await info.attach('profile-sharing-public-receipts', { body: JSON.stringify({ completed: receipts }, null, 2), contentType: 'application/json' });
    await Promise.all(actors.map(actor => actor.context.close()));
  }
});
