import { expect, test, type BrowserContext, type Page, type TestInfo } from '@playwright/test';
import { createHash } from 'node:crypto';
import { completeStartup } from '../helpers';

// Real hosted HTML/JS, genuine founder UI, actual TLS and unmodified HTTP.
// No route fulfillment, app-store writes, fake clipboard or OS registration.
// Target DNS/certificate are isolated browser fixtures; the public site itself
// is reached normally. No installed native dispatch or completed join claim.
const target = process.env.ANNEX_LIVE_URL ?? 'https://annex-invite-check.test:3478';
const digest = (value: string | Buffer) => createHash('sha256').update(value).digest('hex');
type Invite = { code: string; url: string };
type Payload = { server: string; code: string; serverName?: string; description?: string };
type PublicReceipt = {
  status: number; finalHost: string; apexRedirectObserved: boolean; pageSha256: string;
  payloadMatches: boolean; metadataMatches: boolean; noAuthorityHeaders: boolean;
  dispatch?: { exact: boolean; source: 'cdp-request' | 'browser-unregistered-handler'; reason?: string };
  fallbackVisible?: boolean;
};

function checkedPayload(invite: Invite): Payload {
  const url = new URL(invite.url);
  expect(url.origin === 'https://monolithannex.com', 'The real generated link must use the production apex').toBe(true);
  expect(/^\/invite\/[A-Za-z0-9_-]+$/.test(url.pathname), 'The generated route must contain a single base64url payload').toBe(true);
  const parsed = JSON.parse(Buffer.from(url.pathname.slice('/invite/'.length), 'base64url').toString('utf8')) as Payload;
  expect(parsed.server === target, 'The generated payload must identify the actual isolated TLS target').toBe(true);
  expect(parsed.code === invite.code && invite.code.length > 0, 'The generated response and payload must carry the same genuine code').toBe(true);
  return parsed;
}

async function actualPublicPage(page: Page, invite: Invite, payload: Payload, info: TestInfo, label: string): Promise<PublicReceipt> {
  const responses: Array<{ status: number; host: string; path: string }> = [];
  let leakedHeaders = false;
  let finalHTML = '';
  const bodyReads: Promise<void>[] = [];
  page.on('request', request => {
    const url = new URL(request.url());
    if (!['monolithannex.com', 'www.monolithannex.com'].includes(url.hostname)) return;
    const headers = request.headers();
    leakedHeaders ||= Boolean(headers.authorization || headers['x-annex-zk-proof'] || headers['x-annex-pseudonym']);
  });
  page.on('response', response => {
    if (!response.request().isNavigationRequest()) return;
    const url = new URL(response.url());
    if (!['monolithannex.com', 'www.monolithannex.com'].includes(url.hostname)) return;
    responses.push({ status: response.status(), host: url.hostname, path: url.pathname });
    if (response.status() === 200) bodyReads.push(response.text().then(text => { finalHTML = text; }));
  });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Page.enable');
  try {
    // CDP performs a real browser navigation while keeping the bearer URL out
    // of Playwright's human-readable Navigate-to step title and error logs.
    const navigation = await cdp.send('Page.navigate', { url: invite.url });
    expect(!navigation.errorText, 'The actual public invite navigation must succeed').toBe(true);
    await expect(page.getByRole('button', { name: 'Open in Annex', exact: true })).toBeVisible();
    await Promise.all(bodyReads);
    const final = new URL(page.url());
    expect(final.origin === 'https://www.monolithannex.com', 'The actual production apex must redirect to www').toBe(true);
    expect(final.pathname === new URL(invite.url).pathname, 'Redirect must preserve the exact generated payload').toBe(true);
    const data = await page.evaluate(() => ({
      heading: document.querySelector('h1')?.textContent?.trim(),
      server: document.querySelector('.invite-server-url')?.textContent?.trim(),
      description: document.querySelector('.invite-description')?.textContent?.trim() ?? null,
      protocol: document.querySelector('#invite-open-btn')?.getAttribute('data-protocol-url')
        ?? document.querySelector('#invite-open-btn')?.getAttribute('href'),
      ogTitle: document.querySelector('meta[property="og:title"]')?.getAttribute('content'),
      ogDescription: document.querySelector('meta[property="og:description"]')?.getAttribute('content'),
      ogURL: document.querySelector('meta[property="og:url"]')?.getAttribute('content'),
      ogImage: document.querySelector('meta[property="og:image"]')?.getAttribute('content'),
    }));
    const protocol = new URL(data.protocol ?? 'about:blank');
    const payloadMatches = protocol.protocol === 'annex:' && protocol.hostname === 'invite'
      && protocol.searchParams.get('server') === payload.server && protocol.searchParams.get('code') === payload.code
      && data.server === payload.server;
    expect(payloadMatches, 'The actual rendered protocol action must preserve the genuine destination and code').toBe(true);
    const expectedTitle = payload.serverName ?? 'Join an Annex Server';
    const expectedDescription = payload.description ?? 'Sovereign communication. Zero-knowledge identity. Self-hosted.';
    const image = new URL(data.ogImage ?? 'about:blank');
    const metadataMatches = data.heading === expectedTitle && data.ogTitle === expectedTitle
      && data.ogDescription === expectedDescription && data.ogURL === invite.url
      && data.description === (payload.description ?? null)
      && image.origin === 'https://monolithannex.com' && image.pathname === '/api/og'
      && image.searchParams.get('name') === (payload.serverName ?? 'Annex Server')
      && image.searchParams.get('description') === (payload.description ?? '');
    expect(metadataMatches, 'Hosted heading, destination and social metadata must match the generated payload').toBe(true);
    expect(finalHTML.length > 0, 'Retain a digest of the actual hosted response').toBe(true);
    const receipt: PublicReceipt = { status: 200, finalHost: final.hostname,
      apexRedirectObserved: responses.some(item => item.host === 'monolithannex.com' && item.status >= 300 && item.status < 400),
      pageSha256: digest(finalHTML), payloadMatches, metadataMatches, noAuthorityHeaders: !leakedHeaders };
    expect(receipt.apexRedirectObserved, 'Observe the real HTTP redirect rather than guessing from a final URL').toBe(true);
    expect(receipt.noAuthorityHeaders, 'No Annex credentials may be sent to the public router').toBe(true);
    cdp.on('Page.frameRequestedNavigation', (event: { url: string; reason?: string }) => {
      if (!event.url.startsWith('annex:')) return;
      receipt.dispatch = { exact: event.url === protocol.href, source: 'cdp-request', reason: event.reason };
    });
    page.on('console', message => {
      const text = message.text();
      if (!receipt.dispatch && text.includes(protocol.href)
        && /Failed to launch|not have a registered handler|no registered handler/i.test(text)) {
        // Retain the result, never the browser message containing the code.
        receipt.dispatch = { exact: true, source: 'browser-unregistered-handler' };
      }
    });
    await page.getByRole('button', { name: 'Open in Annex', exact: true }).click();
    await expect.poll(() => receipt.dispatch?.exact === true, {
      message: 'Observe the actual protocol-navigation attempt or explicit unregistered-handler refusal', timeout: 10_000,
    }).toBe(true);
    // This host has no registered Annex scheme. Do not approve an external-app
    // prompt or click any download. The website must reveal its real fallback.
    await expect(page.locator('#invite-fallback')).toHaveClass(/\bvisible\b/, { timeout: 8_000 });
    await expect(page.locator('#invite-fallback')).toBeVisible();
    receipt.fallbackVisible = true;
    expect(leakedHeaders, 'Protocol attempt must not leak Annex HTTP credentials to the site').toBe(false);
    const visibleText = await page.locator('body').innerText();
    expect(visibleText.includes(invite.code) || visibleText.includes(invite.url)
      || visibleText.includes(new URL(invite.url).pathname), 'Visible landing content must not contain the bearer code or encoded invite before capture').toBe(false);
    await info.attach(`${label}-public-landing-page`, {
      body: await page.locator('body').screenshot(), contentType: 'image/png',
    });
    return receipt;
  } catch {
    // Avoid serializing a raw navigation/DOM exception that includes the code.
    throw new Error('Actual public invite routing, metadata, dispatch or fallback check failed; bearer details withheld');
  } finally {
    finalHTML = '';
    await cdp.detach();
  }
}

test('actual generated invite: production website routing, protocol attempt and real HTTPS validation (native join remains unqualified)', async ({ browser }, info) => {
  expect(new URL(target).origin === 'https://annex-invite-check.test:3478', 'Use the dedicated genuine TLS fixture').toBe(true);
  expect(/^[A-Za-z0-9+/]{43}=$/.test(process.env.ANNEX_INVITE_TLS_SPKI ?? ''), 'Require the exact fixture certificate SPKI, never a blanket TLS bypass').toBe(true);
  const contexts: BrowserContext[] = [];
  const receipts: Record<string, unknown> = {
    scope: 'Actual public marketing route and unmodified local TLS validation only; no installed native dispatch or completed server join',
    targetDNS: 'annex-invite-check.test resolves to loopback only in this test browser',
    targetPublicReachability: false, nativeHandlerRegistered: false,
    sourceCrossReference: { repository: 'monolith-annex', deploymentCommit: '2d624d8',
      verifiedSeparately: 'Site snapshot and live JS/platform/CSS receipts; this test independently hashes its actual hosted response' },
  };
  let invite: Invite | undefined;
  const founderHeaders: Record<string, string> = {};
  try {
    const founder = await browser.newContext({ baseURL: target }); contexts.push(founder);
    await founder.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: target });
    const page = await founder.newPage();
    page.on('request', request => {
      if (new URL(request.url()).origin !== target) return;
      const headers = request.headers();
      if (headers.authorization && headers['x-annex-zk-proof']) for (const [name, value] of Object.entries(headers)) {
        if (name === 'authorization' || name.startsWith('x-annex-')) founderHeaders[name] = value;
      }
    });
    await test.step('genuine founder startup, automatic server invite and real Copy Link clipboard', async () => {
      await completeStartup(page);
      await page.locator('.admin-menu-btn').click();
      const generated = page.waitForResponse(response => response.request().method() === 'POST'
        && new URL(response.url()).origin === target && new URL(response.url()).pathname === '/api/invites');
      await page.getByRole('button', { name: 'Server Settings', exact: true }).click();
      const response = await generated;
      expect(response.status(), 'Real invite creation must succeed').toBe(200);
      invite = await response.json() as Invite;
      checkedPayload(invite);
      const input = page.getByRole('textbox', { name: 'Invite link', exact: true });
      await expect(input).toBeVisible();
      expect(await input.inputValue() === invite.url, 'Displayed link must equal the genuine server-issued URL').toBe(true);
      await page.getByRole('button', { name: 'Copy Link', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Copied!', exact: true })).toBeVisible();
      expect(await page.evaluate(() => navigator.clipboard.readText()) === invite.url, 'Actual clipboard must contain the displayed generated link').toBe(true);
      receipts.inviteURLSha256 = digest(invite.url); receipts.inviteCodeSha256 = digest(invite.code);
      receipts.generatedAndCopied = true;
    });
    if (!invite) throw new Error('Genuine invite was not captured');
    const captured = invite;
    const payload = checkedPayload(captured);
    await test.step('founder follows the actual public apex and clicks the exact rendered protocol action', async () => {
      const hosted = await founder.newPage();
      receipts.founderPublicRoute = await actualPublicPage(hosted, captured, payload, info, 'founder');
      await hosted.close();
    });
    const recipient = await browser.newContext({ baseURL: target }); contexts.push(recipient);
    await test.step('fresh recipient with no cookies or identity sees the same genuine route and actual download fallback', async () => {
      expect((await recipient.cookies()).length).toBe(0);
      const hosted = await recipient.newPage();
      receipts.recipientPublicRoute = await actualPublicPage(hosted, captured, payload, info, 'recipient');
      await hosted.close();
    });
    await test.step('exact emitted code validates at the genuine TLS target; wrong code refuses without creating a recipient identity', async () => {
      const targetPage = await recipient.newPage();
      let authorityHeaders = false; let registrations = 0;
      targetPage.on('request', request => {
        const url = new URL(request.url());
        if (url.origin !== target) return;
        const headers = request.headers();
        authorityHeaders ||= Boolean(headers.authorization || headers['x-annex-zk-proof']);
        if (request.method() === 'POST' && url.pathname === '/api/registry/register') registrations += 1;
      });
      await targetPage.goto('/');
      await expect(targetPage.getByRole('button', { name: 'Create New Identity', exact: true })).toBeVisible();
      const result = await targetPage.evaluate(async ({ code }) => {
        const check = async (value: string) => {
          const response = await fetch('/api/invites/redeem', { method: 'POST', credentials: 'omit',
            headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ code: value }) });
          const body = await response.json() as { valid?: boolean; serverName?: string; serverSlug?: string; error?: string };
          return { status: response.status, valid: body.valid === true, serverName: body.serverName, serverSlug: body.serverSlug,
            invalidMessage: body.error === 'Invalid or expired invite code' };
        };
        return { accepted: await check(code), refused: await check(`${code}-invalid`) };
      }, { code: payload.code });
      expect(result.accepted.status).toBe(200); expect(result.accepted.valid).toBe(true);
      expect(result.accepted.serverName === payload.serverName, 'Redemption must identify the same actual server').toBe(true);
      expect(Boolean(result.accepted.serverSlug)).toBe(true);
      expect(result.refused.status).toBe(400); expect(result.refused.valid).toBe(false); expect(result.refused.invalidMessage).toBe(true);
      expect(authorityHeaders, 'The recipient validation request must carry no existing identity credentials').toBe(false);
      expect(registrations, 'Validation alone is not registration').toBe(0);
      await expect(targetPage.getByRole('button', { name: 'Create New Identity', exact: true })).toBeVisible();
      const count = await page.evaluate(async ({ headers, code }) => {
        const response = await fetch('/api/invites', { headers });
        if (!response.ok) return { status: response.status, found: false, count: -1 };
        const entries = await response.json() as Array<{ code: string; useCount: number }>;
        const entry = entries.find(item => item.code === code);
        return { status: response.status, found: Boolean(entry), count: entry?.useCount ?? -1 };
      }, { headers: founderHeaders, code: payload.code });
      expect(count.status).toBe(200); expect(count.found).toBe(true); expect(count.count).toBe(0);
      receipts.validation = { genuineCode: 200, wrongCode: 400, recipientRegistrations: 0, useCount: count.count, noCredentials: !authorityHeaders };
    });
    await test.step('actual malformed and private-destination public routes refuse without reflecting the supplied code or contacting target', async () => {
      const sentinel = `NONCREDENTIAL_REFLECTION_CHECK_${Date.now()}`;
      const privatePayload = Buffer.from(JSON.stringify({ server: 'https://127.0.0.1:3478', code: sentinel })).toString('base64url');
      const cases = ['not-valid-json', privatePayload];
      const negatives = [];
      for (const path of cases) {
        const negative = await recipient.newPage(); let contactedTarget = false;
        negative.on('request', request => {
          const host = new URL(request.url()).hostname;
          if (host === '127.0.0.1' || host === 'annex-invite-check.test') contactedTarget = true;
        });
        const response = await negative.goto(`https://monolithannex.com/invite/${path}`);
        expect(response?.status()).toBe(400);
        await expect(negative.getByRole('heading', { name: 'Invalid Invite', exact: true })).toBeVisible();
        await expect(negative.locator('#invite-open-btn')).toHaveCount(0);
        const html = await negative.content();
        expect(html.includes(sentinel), 'Invalid response must not reflect the supplied code').toBe(false);
        expect(contactedTarget, 'The public router must not contact a refused destination').toBe(false);
        negatives.push({ status: 400, reflectedCode: false, contactedTarget: false, responseSha256: digest(html) });
        await negative.close();
      }
      receipts.negativeRoutes = negatives;
    });
  } finally {
    invite = undefined;
    for (const key of Object.keys(founderHeaders)) delete founderHeaders[key];
    await info.attach('redacted-invite-routing-evidence', { body: JSON.stringify(receipts, null, 2), contentType: 'application/json' });
    for (const context of contexts) await context.close().catch(() => {});
  }
});
