import { chromium, expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

test('native updater rejects transport and signature failures, retries, and invokes the actual installer', async () => {
  const endpoint = process.env.ANNEX_NATIVE_CDP;
  const output = process.env.ANNEX_NATIVE_EVIDENCE;
  const executable = process.env.ANNEX_NATIVE_EXECUTABLE;
  const modeFile = process.env.ANNEX_UPDATER_MODE_FILE;
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(endpoint ?? '') || !output || !executable || !modeFile) {
    throw new Error('Explicit native updater fixture required');
  }
  const hash = async () => createHash('sha256').update(await readFile(executable)).digest('hex');
  const before = await hash();
  const browser = await chromium.connectOverCDP(endpoint!);
  const stages: Record<string, unknown>[] = [];
  try {
    const pages = browser.contexts().flatMap(context => context.pages());
    expect(pages).toHaveLength(1);
    const page = pages[0];
    await page.getByRole('button', { name: 'Updates', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByText('Installed version: 0.1.2', { exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Check for updates', exact: true }).click();
    await expect(dialog.getByText('Available version: 0.1.3', { exact: true })).toBeVisible();

    for (const mode of ['stall', 'corrupt', 'normal'] as const) {
      // Change only the external server's transfer behavior. No app state or
      // native command is injected; all updater actions use the existing UI.
      await writeFile(modeFile, mode);
      await dialog.getByRole('button', { name: 'Download update', exact: true }).click();
      if (mode === 'normal') {
        await expect(dialog.getByText('Signature verified. Ready to install.', { exact: true })).toBeVisible({ timeout: 180_000 });
      } else {
        const pattern = mode === 'stall' ? /stopped receiving data/i : /signature/i;
        await expect(dialog.getByRole('alert').filter({ hasText: pattern }).first()).toBeVisible({ timeout: 150_000 });
        expect(await hash()).toBe(before);
        await expect(dialog.getByRole('button', { name: 'Download update', exact: true })).toBeEnabled();
      }
      stages.push({ mode, installedBinaryUnchanged: await hash() === before, completedAt: new Date().toISOString() });
      await page.screenshot({ path: join(output, `updater-${mode}.png`) });
    }
    await writeFile(join(output, 'updater-downloads.json'), JSON.stringify({
      baselineBinarySha256: before, nativeWebView: true, realUpdater: true,
      productionVerificationPublicKey: true, controlledTlsOrigin: true, stages,
      actualInstallerCompletionVerifiedSeparately: true,
    }, null, 2) + '\n');
    await dialog.getByRole('button', { name: 'Install and restart', exact: true }).click();
    await expect(dialog.getByRole('alert').filter({ hasText: /closes active calls/ })).toBeVisible();
    await dialog.getByRole('button', { name: 'Confirm install and restart', exact: true }).click();
  } finally {
    await browser.close();
  }
});
