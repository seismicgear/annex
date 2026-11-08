import { expect, test, type Page } from '@playwright/test';
import { captureDiagnostic, clearMaskLocators, maskLocators } from './audit/nav';

const MASK = [58, 58, 58, 255];
const WHITE = [255, 255, 255, 255];

async function pixels(page: Page, png: Buffer, points: number[][]): Promise<number[][]> {
  return page.evaluate(async ({ base64, points }) => {
    const image = new Image();
    image.src = 'data:image/png;base64,' + base64;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d')!;
    context.drawImage(image, 0, 0);
    return points.map(([x, y]) => [...context.getImageData(x, y, 1, 1).data]);
  }, { base64: png.toString('base64'), points });
}

async function capture(page: Page, clip?: string, extra: string[] = []): Promise<Buffer> {
  try {
    const mask = await maskLocators(page, extra, clip);
    const target = clip ? page.locator(clip) : page;
    return await target.screenshot({ mask, maskColor: '#3a3a3a', animations: 'disabled' });
  } finally {
    await clearMaskLocators(page);
    await expect(page.locator('[data-annex-audit-mask-root]')).toHaveCount(0);
  }
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 400 });
});

test('audit masks cannot paint scrolled-off metadata over controls and retain partial fragments', async ({ page }) => {
  await page.setContent(`<style>
    body { margin:0; background:white; }
    #control { position:absolute; left:20px; top:20px; width:200px; height:60px; background:rgb(0,180,0); }
    #scroll { position:absolute; left:20px; top:120px; width:200px; height:160px; overflow:auto; border:4px solid #333; }
    #content { position:relative; height:400px; }
    .timestamp,.sender { position:absolute; left:12px; width:72px; height:20px; background:red; }
    </style><button id="control">Enable encryption</button><div id="scroll"><div id="content">
    <span class="timestamp" style="top:10px">hidden</span>
    <span class="sender" style="top:88px">partial</span>
    <span class="timestamp" style="top:140px">visible</span>
    </div></div>`);
  await page.locator('#scroll').evaluate(el => { el.scrollTop = 100; });
  const screenshot = await capture(page);
  expect(await pixels(page, screenshot, [[40, 44], [40, 116], [40, 122], [40, 126], [40, 174]]))
    .toEqual([[0, 180, 0, 255], WHITE, [51, 51, 51, 255], MASK, MASK]);
});

test('audit masks intersect nested overflow independently on each clipping axis', async ({ page }) => {
  await page.setContent(`<style>
    body { margin:0; background:white; }
    #outer { position:absolute; left:80px; top:80px; width:160px; height:80px; overflow-x:visible; overflow-y:clip; }
    #inner { position:absolute; left:-20px; top:-20px; width:120px; height:140px; overflow-x:clip; overflow-y:visible; }
    .volatile { position:absolute; left:-10px; top:-10px; width:160px; height:140px; background:red; }
    </style><div id="outer"><div id="inner"><span class="volatile">volatile</span></div></div>`);
  const screenshot = await capture(page, undefined, ['.volatile']);
  expect(await pixels(page, screenshot, [[65, 100], [55, 100], [70, 70], [170, 155], [181, 120]]))
    .toEqual([MASK, WHITE, WHITE, MASK, WHITE]);
});

test('audit masks preserve locator subject scope after screenshot scrolling', async ({ page }) => {
  await page.setContent(`<style>
    body { margin:0; height:1200px; background:white; }
    #subject { position:absolute; left:80px; top:700px; width:160px; height:80px; background:rgb(0,180,0); }
    .sender { position:absolute; left:20px; top:10px; width:72px; height:20px; background:red; }
    #outside { left:0; top:0; }
    </style><span class="sender" id="outside">outside</span><div id="subject"><span class="sender">inside</span></div>`);
  const screenshot = await capture(page, '#subject >> nth=0');
  expect(await pixels(page, screenshot, [[25, 15], [120, 45]])).toEqual([MASK, [0, 180, 0, 255]]);
  expect((await page.locator('#subject').boundingBox())!.y).toBeLessThan(400);
  await page.evaluate(() => scrollTo(0, 0));
  const outside = await page.screenshot();
  expect(await pixels(page, outside, [[3, 3]])).toEqual([[255, 0, 0, 255]]);
});

test('audit masks include the subject itself and stop at viewport edges', async ({ page }) => {
  await page.setContent(`<style>body {margin:0; background:white;}
    #subject {position:absolute;left:20px;top:30px;width:72px;height:20px;background:red;}
    </style><span class="sender" id="subject">volatile</span>`);
  expect(await pixels(page, await capture(page, '#subject >> nth=0'), [[4, 4], [70, 18]]))
    .toEqual([MASK, MASK]);
  await page.setContent(`<style>html {overflow:clip;} body {margin:0;background:white;}
    .sender {position:absolute;left:390px;top:390px;width:40px;height:40px;background:red;}
    </style><span class="sender">edge</span>`);
  try {
    const mask = await maskLocators(page);
    const bounds = (await Promise.all(mask.map(locator => locator.evaluateAll(elements =>
      elements.map(element => {
        const r = element.getBoundingClientRect();
        return { x: r.x, y: r.y, width: r.width, height: r.height };
      }),
    )))).flat();
    expect(bounds).toEqual([{ x: 390, y: 390, width: 10, height: 10 }]);
    const screenshot = await page.screenshot({ mask, maskColor: '#3a3a3a' });
    expect(await pixels(page, screenshot, [[395, 395], [385, 395]])).toEqual([MASK, WHITE]);
  } finally {
    await clearMaskLocators(page);
  }
});

test('audit diagnostics fall back when mask preparation cannot find the subject', async ({ page }) => {
  await page.setContent('<style>body{margin:0;background:rgb(0,180,0)}</style><span class="sender">volatile</span>');
  await maskLocators(page);
  await expect(page.locator('[data-annex-audit-mask-root]')).toHaveCount(1);
  page.setDefaultTimeout(100);
  const screenshot = await captureDiagnostic(page, test.info().outputPath('missing-subject.png'), [], '#missing');
  expect(screenshot).toBeDefined();
  expect(await pixels(page, screenshot!, [[100, 100]])).toEqual([[0, 180, 0, 255]]);
  await expect(page.locator('[data-annex-audit-mask-root]')).toHaveCount(0);
});
