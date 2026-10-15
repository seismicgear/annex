#!/usr/bin/env node
// Qualify the Linux screenshot environment before recording or comparing images.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const receiptPath = path.join(root, 'docs/ui-audit/font-environment.json');
const receipt = { checkedAt: new Date().toISOString(), platform: process.platform,
  fontconfigFile: process.env.FONTCONFIG_FILE ?? null, fontconfigPath: process.env.FONTCONFIG_PATH ?? null,
  qualified: false, fonts: [] };
let browser;
try {
  if (process.platform !== 'linux') throw new Error('The committed UI audit references require the Linux capture environment.');
  if (!process.env.PLAYWRIGHT_BROWSERS_PATH) {
    try { if ((await fs.stat('/opt/pw-browsers')).isDirectory()) process.env.PLAYWRIGHT_BROWSERS_PATH = '/opt/pw-browsers'; }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  const require = createRequire(path.join(root, 'client/package.json'));
  const { chromium } = require('@playwright/test');
  browser = await chromium.launch({ headless: true });
  receipt.browser = await browser.version();
  const page = await browser.newPage();
  await page.setContent('<p id="regular">Annex font inventory 0123456789</p><p id="italic" class="voice-setup-hint">Annex font inventory 0123456789</p>');
  await page.addStyleTag({ path: path.join(root, 'client/src/index.css') });
  await page.addStyleTag({ path: path.join(root, 'client/src/App.css') });
  await page.evaluate(() => document.fonts.ready);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('DOM.enable'); await cdp.send('CSS.enable');
  const documentNode = await cdp.send('DOM.getDocument');
  for (const [id, postscript, pattern] of [
    ['regular', 'DejaVuSans', 'system-ui:slant=roman'],
    ['italic', 'DejaVuSans-Oblique', 'system-ui:slant=italic'],
  ]) {
    const node = await cdp.send('DOM.querySelector', { nodeId: documentNode.root.nodeId, selector: `#${id}` });
    const rendered = (await cdp.send('CSS.getPlatformFontsForNode', { nodeId: node.nodeId })).fonts;
    const [matchedName, filename] = execFileSync('fc-match', ['-f', '%{postscriptname}\t%{file}', pattern], { encoding: 'utf8' }).trim().split('\t');
    const font = { role: id, expectedPostscriptName: postscript, rendered, matchedName, filename };
    receipt.fonts.push(font);
    if (matchedName !== postscript || rendered.length !== 1 || rendered[0].postScriptName !== postscript || rendered[0].glyphCount < 1) {
      throw new Error(`UI audit ${id} font is not ${postscript}. Install fontconfig fonts-dejavu-core fonts-dejavu-extra, then rerun the font check. Missing italic faces must not be recorded as upright text.`);
    }
    const bytes = await fs.readFile(filename);
    font.sha256 = createHash('sha256').update(bytes).digest('hex');
    font.bytes = bytes.length;
  }
  receipt.qualified = true;
  console.log(`[ui-audit-fonts] qualified ${receipt.fonts.map(font => font.matchedName).join(', ')} with Chromium ${receipt.browser}`);
} catch (error) {
  receipt.error = error.message;
  console.error(`[ui-audit-fonts] ${error.message}`);
  process.exitCode = 1;
} finally {
  await browser?.close();
  await fs.mkdir(path.dirname(receiptPath), { recursive: true });
  await fs.writeFile(receiptPath, JSON.stringify(receipt, null, 2) + '\n');
}
