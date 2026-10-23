import { expect, test, type Locator } from '@playwright/test';
import { baseURL, publicIdentities, read, row, select } from './operator-helpers';
import { bubble, createText, encryptionActor, ready, sendEncrypted, signedEvidence, trust, type EncryptionActor } from './encryption-helpers';
import type { ManifestPage, ManifestOperation } from '../../src/lib/e2e-manifest';
import type { ChannelKeyState } from '../../src/lib/e2e-v2-protocol';

test('signed channel roles, private admission and member removal govern real browser access', async ({ browser }) => {
  const actors: EncryptionActor[] = [];
  const receipts: Record<string, unknown> = {};
  const founder = await encryptionActor(browser); actors.push(founder);
  try {
    await expect(founder.page.locator('.admin-menu-btn'), 'A fresh dedicated database is required').toBeVisible();
    const member = await encryptionActor(browser); actors.push(member);
    const guest = await encryptionActor(browser); actors.push(guest);
    const founderId = (await publicIdentities(founder))[0].pseudonymId!;
    const memberId = (await publicIdentities(member))[0].pseudonymId!;
    const guestId = (await publicIdentities(guest))[0].pseudonymId!;
    const name = `signed-access-${Date.now()}`;
    const channel = await createText(founder, name);
    const path = `/api/channels/${channel.channel_id}`;
    const manifest = () => read<ManifestPage>(founder, `${path}/key-manifest`);
    const state = () => read<ChannelKeyState>(founder, `${path}/key-status`);
    const reconcile = async (actor: EncryptionActor) => {
      await select(actor, name);
      const retry = actor.page.getByRole('region', { name: 'Channel encryption', exact: true }).getByRole('button', { name: /^(Check again|Retry)$/ });
      if (await retry.isVisible()) await retry.click();
      await ready(actor);
    };
    const access = async (actor: EncryptionActor) => {
      await actor.page.getByRole('button', { name: 'Encryption access', exact: true }).click();
      const dialog = actor.page.getByRole('dialog', { name: 'Encryption access', exact: true });
      await expect(dialog).toBeVisible();
      return dialog;
    };
    const close = async (dialog: Locator) => { await dialog.getByRole('button', { name: 'Done', exact: true }).click(); await expect(dialog).toHaveCount(0); };
    const memberRow = (dialog: Locator, id: string) => dialog.locator(`[data-encryption-member="${id}"]`);
    const signedAction = async (actor: EncryptionActor, action: () => Promise<unknown>, expected: Partial<ManifestOperation>) => {
      const pending = actor.page.waitForResponse(response => response.request().method() === 'POST'
        && new URL(response.url()).pathname === `${path}/key-manifest`);
      void pending.catch(() => {});
      await action(); expect((await pending).status()).toBe(200);
      await expect.poll(async () => (await manifest()).events.at(-1)?.operation).toMatchObject(expected);
    };
    const confirmRole = async (actor: EncryptionActor, target: string, role: 'member' | 'moderator', cancelFirst = false) => {
      const dialog = await access(actor);
      const button = memberRow(dialog, target).getByRole('button', { name: role === 'member' ? 'Make member' : 'Make moderator', exact: true });
      const before = (await manifest()).head_hash;
      await button.click();
      const confirmation = dialog.getByRole('region', { name: 'Confirm encryption access change', exact: true });
      await expect(confirmation.locator('code')).toHaveText(target);
      if (cancelFirst) {
        await confirmation.getByRole('button', { name: 'Cancel change', exact: true }).click();
        expect((await manifest()).head_hash).toBe(before);
        await button.click();
      }
      await signedAction(actor, () => confirmation.getByRole('button', { name: 'Confirm change', exact: true }).click(), { kind: 'set_role', pseudonym_id: target, role });
      await expect(confirmation).toHaveCount(0); await close(dialog);
    };
    const admit = async (actor: EncryptionActor, target: EncryptionActor, targetId: string) => {
      const dialog = await access(actor);
      const input = dialog.getByLabel('Identity pseudonym', { exact: true });
      await input.fill('invalid'); await expect(dialog.getByRole('button', { name: 'Load certified devices', exact: true })).toBeDisabled();
      await input.fill(targetId); await dialog.getByRole('button', { name: 'Load certified devices', exact: true }).click();
      await expect.poll(() => target.deviceId).toMatch(/^[0-9a-f]{64}$/);
      await dialog.getByRole('radio', { name: new RegExp(target.deviceId!) }).check();
      await signedAction(actor, () => dialog.getByRole('button', { name: 'Admit member', exact: true }).click(), { kind: 'admit_member' });
      await expect(memberRow(dialog, targetId)).toContainText('Channel member');
      await dialog.getByRole('button', { name: 'Refresh access', exact: true }).click();
      await expect(memberRow(dialog, targetId)).toBeVisible(); await close(dialog);
    };

    await test.step('Initialize signed membership and compare displayed verified fingerprints', async () => {
      await select(founder, name); await select(member, name);
      await founder.page.getByRole('button', { name: /Enable encryption/ }).click(); await ready(founder);
      await select(member, name); await trust(member, founder, channel.channel_id);
      await reconcile(founder); await reconcile(member);
      const original = await signedEvidence(founder, channel.channel_id);
      const dialog = await access(member);
      await expect(dialog.locator('dl code')).toHaveText([original.page.authority_identity, original.page.genesis_hash!]);
      await expect(dialog.getByLabel('Admission policy', { exact: true })).toBeDisabled();
      await expect(dialog.getByRole('button', { name: 'Make moderator', exact: true })).toHaveCount(0); await close(dialog);
      await sendEncrypted(founder, channel.channel_id, 'history-before-private-admission');
      await expect(bubble(member, 'history-before-private-admission')).toBeVisible();
      receipts.initial = original.publicReceipt;
    });

    await test.step('Private policy prevents self-admission until a moderator approves a certified device', async () => {
      const dialog = await access(founder);
      await expect(dialog.getByRole('button', { name: 'Save admission policy', exact: true })).toBeDisabled();
      await dialog.getByLabel('Admission policy', { exact: true }).selectOption('moderator_admission');
      await signedAction(founder, () => dialog.getByRole('button', { name: 'Save admission policy', exact: true }).click(), { kind: 'set_policy', join_policy: 'moderator_admission' });
      await close(dialog);
      await select(guest, name); await trust(guest, founder, channel.channel_id);
      await expect(guest.page.getByRole('region', { name: 'Channel encryption', exact: true })).toContainText(/moderator.*admit/i);
      const sent = guest.sent.length;
      await guest.page.getByPlaceholder('Type a message...').fill('private-unadmitted-must-not-leak');
      await expect(guest.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      // The current composer prevents pointer submission before a request.
      // Exercise keyboard submission too: it must not bypass private admission.
      await guest.page.getByPlaceholder('Type a message...').press('Enter');
      await expect(guest.page.getByRole('region', { name: 'Channel encryption', exact: true })).toContainText(/moderator.*admit/i);
      await expect(guest.page.getByPlaceholder('Type a message...')).toHaveValue('private-unadmitted-must-not-leak');
      expect(guest.sent).toHaveLength(sent); await guest.page.getByPlaceholder('Type a message...').fill('');
      expect((await signedEvidence(founder, channel.channel_id)).directory.member_keys.some(key => key.pseudonym_id === guestId)).toBe(false);
      await admit(founder, guest, guestId); await reconcile(founder); await reconcile(guest);
      await expect(bubble(guest, 'history-before-private-admission')).toBeVisible();
      receipts.privateAdmission = (await signedEvidence(guest, channel.channel_id)).publicReceipt;
    });

    await test.step('The last moderator cannot demote itself; promotion gives authority without global admin', async () => {
      const dialog = await access(founder);
      const before = (await manifest()).head_hash;
      await memberRow(dialog, founderId).getByRole('button', { name: 'Make member', exact: true }).click();
      const confirmation = dialog.getByRole('region', { name: 'Confirm encryption access change', exact: true });
      await confirmation.getByRole('button', { name: 'Confirm change', exact: true }).click();
      await expect(dialog.getByRole('alert')).toContainText(/moderator/i);
      expect((await manifest()).head_hash).toBe(before);
      await confirmation.getByRole('button', { name: 'Cancel change', exact: true }).click(); await close(dialog);
      await confirmRole(founder, memberId, 'moderator', true); await reconcile(member);
      await expect(member.page.locator('.admin-menu-btn')).toHaveCount(0);
      await expect(member.page.getByRole('button', { name: 'Disable encryption', exact: true })).toBeVisible();
      receipts.promotion = (await signedEvidence(member, channel.channel_id)).publicReceipt;
    });

    await test.step('Signed demotion removes encryption authority even from the server founder', async () => {
      await confirmRole(member, founderId, 'member'); await reconcile(founder);
      await expect(founder.page.locator('.admin-menu-btn')).toBeVisible();
      await expect(founder.page.getByRole('button', { name: 'Disable encryption', exact: true })).toHaveCount(0);
      const dialog = await access(founder);
      await expect(dialog.getByLabel('Admission policy', { exact: true })).toBeDisabled();
      await expect(dialog.getByRole('button', { name: 'Save admission policy', exact: true })).toHaveCount(0);
      await expect(dialog.getByRole('button', { name: 'Remove member', exact: true })).toHaveCount(0); await close(dialog);
      await confirmRole(member, founderId, 'moderator'); await reconcile(founder);
      receipts.demotion = { globalFounderStayedAdmin: true, signedRoleRemovedControls: true };
    });

    await test.step('Confirmed removal ends actual membership and access; explicit rejoin still requires admission', async () => {
      const dialog = await access(founder);
      const target = memberRow(dialog, guestId);
      const before = await state(); const head = (await manifest()).head_hash;
      await target.getByRole('button', { name: 'Remove member', exact: true }).click();
      const confirmation = dialog.getByRole('region', { name: 'Confirm encryption access change', exact: true });
      await confirmation.getByRole('button', { name: 'Cancel change', exact: true }).click();
      expect((await manifest()).head_hash).toBe(head);
      await target.getByRole('button', { name: 'Remove member', exact: true }).click();
      await signedAction(founder, () => confirmation.getByRole('button', { name: 'Confirm change', exact: true }).click(), { kind: 'remove_member', pseudonym_id: guestId });
      await expect(target).toHaveCount(0); await close(dialog);
      await expect(row(guest, name).locator('.join-btn')).toBeVisible();
      await expect(guest.page.getByPlaceholder('Type a message...')).toHaveCount(0);
      for (const route of ['/messages', '/member-keys', '/key-status', '/key-manifest']) {
        const response = await guest.context.request.get(new URL(path + route, baseURL).href, { headers: guest.headers });
        expect(response.status(), `Removed identity ${route}`).toBe(403);
      }
      await reconcile(founder); await reconcile(member);
      expect((await state()).current_epoch).toBeGreaterThan(before.current_epoch);
      await sendEncrypted(founder, channel.channel_id, 'after-signed-member-removal');
      await expect(bubble(member, 'after-signed-member-removal')).toBeVisible();
      await expect(row(guest, name).locator('.join-btn')).toBeVisible();
      await expect(bubble(guest, 'after-signed-member-removal')).toHaveCount(0);
      await select(guest, name);
      await expect(guest.page.getByRole('region', { name: 'Channel encryption', exact: true })).toContainText(/moderator.*admit/i);
      await admit(founder, guest, guestId); await reconcile(founder); await reconcile(guest);
      await expect(bubble(guest, 'after-signed-member-removal')).toBeVisible();
      receipts.removal = { ...(await signedEvidence(guest, channel.channel_id)).publicReceipt,
        canceledRemovalPreservedHead: true, removedReadsDenied: true, noAutomaticRejoin: true, explicitPrivateRejoinRequiredAdmission: true };
    });

    await test.step('An authorized moderator restores eligible self-join and the signed history remains verifiable', async () => {
      const dialog = await access(member);
      await dialog.getByLabel('Admission policy', { exact: true }).selectOption('eligible_self_join');
      await signedAction(member, () => dialog.getByRole('button', { name: 'Save admission policy', exact: true }).click(), { kind: 'set_policy', join_policy: 'eligible_self_join' });
      await close(dialog); await reconcile(founder); await reconcile(guest);
      const text = `after-governance-${Date.now()}`;
      await sendEncrypted(guest, channel.channel_id, text); await expect(bubble(founder, text)).toBeVisible();
      receipts.final = (await signedEvidence(founder, channel.channel_id)).publicReceipt;
    });
  } finally {
    await test.info().attach('signed-governance-evidence', { body: JSON.stringify(receipts, null, 2), contentType: 'application/json' });
    for (const actor of actors) {
      await test.info().attach('public-governance-progress', { body: JSON.stringify({ cryptoRequests: actor.cryptoRequests,
        encryptionControl: await actor.page.getByRole('region', { name: 'Channel encryption', exact: true }).allTextContents(),
        dialogAlerts: await actor.page.getByRole('dialog').getByRole('alert').allTextContents() }), contentType: 'application/json' });
      expect.soft(actor.errors).toEqual([]); await actor.context.close();
    }
  }
});
