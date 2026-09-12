// Optional browser acceptance test. Requires Node >=22 and a Chromium-based browser.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { checkLayout } from './layout-checks.mjs';

const profile = await mkdtemp(join(tmpdir(), 'slidown-browser-'));
const browser = spawn(process.env.BROWSER || 'microsoft-edge', [
  '--headless', '--no-sandbox', '--disable-gpu', '--disable-background-networking',
  '--disable-component-update', '--no-first-run', '--no-default-browser-check',
  '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank',
], { stdio: ['ignore', 'ignore', 'pipe'] });
let socket;
let server;
let liveDirectory;
try {
  const endpoint = await new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error('Browser startup timed out')), 15000);
    browser.on('error', reject);
    browser.stderr.on('data', data => {
      output += data;
      const match = /DevTools listening on (ws:\/\/[^\s]+)/.exec(output);
      if (match) { clearTimeout(timer); resolve(match[1]); }
    });
    browser.on('exit', code => { clearTimeout(timer); reject(new Error(`Browser exited ${code}: ${output.slice(-2000)}`)); });
  });
  socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let sequence = 0;
  const pending = new Map();
  const exceptions = [];
  const requests = [];
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.id) {
      const p = pending.get(message.id);
      if (p) { clearTimeout(p.timer); pending.delete(message.id); message.error ? p.reject(new Error(JSON.stringify(message.error))) : p.resolve(message.result); }
    }
    if (message.method === 'Runtime.exceptionThrown') exceptions.push(message.params.exceptionDetails);
    if (message.method === 'Network.requestWillBeSent') requests.push(message.params.request.url);
  };
  function call(method, params = {}, sessionId) {
    const id = ++sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 10000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }
  const { targetId } = await call('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
  const cdp = (method, params) => call(method, params, sessionId);
  async function evaluate(expression) {
    const r = await cdp('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
    return r.result.value;
  }
  async function ready() {
    for (let n = 0; n < 100; n++) {
      // External example images must not block checks of the local slide UI.
      if (await evaluate("document.readyState !== 'loading' && !!document.querySelector('.slide.active') && [...document.images].filter(img => !/^https?:/.test(img.src)).every(img => img.complete)")) {
        await evaluate('document.fonts.ready.then(() => true)');
        await delay(150); return;
      }
      await delay(50);
    }
    throw new Error('Page did not become ready');
  }
  await cdp('Runtime.enable');
  await cdp('Network.enable');
  await cdp('Page.enable');
  await cdp('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await cdp('Page.navigate', { url: pathToFileURL(resolve(process.argv[2] || 'dist/index.html')).href });
  await ready();
  const output = resolve('target/browser-validation');
  await mkdir(output, { recursive: true });
  // Shrink and expand at a fixed height: code must stay inside its background,
  // and scaled paragraphs must still use the available slide width.
  await evaluate("location.hash = '/8'");
  await delay(350);
  const originalCode = await evaluate("[...document.querySelectorAll('.slide.active pre')].map(pre => pre.textContent)");
  for (const width of [1600, 1440, 1320, 1200, 1080, 960, 800, 600, 390, 600, 960, 1200, 1600]) {
    await cdp('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: false });
    await delay(150);
    const layout = await evaluate(`(() => {
      const slide = document.querySelector('.slide.active');
      const inner = slide.querySelector('.inner');
      const css = getComputedStyle(slide);
      const available = slide.clientWidth - parseFloat(css.paddingLeft) - parseFloat(css.paddingRight);
      const rect = inner.getBoundingClientRect();
      return {
        width: rect.width, available,
        bottom: rect.bottom, limit: slide.clientHeight - parseFloat(css.paddingBottom),
        overflowingCode: [...inner.querySelectorAll('pre')].some(pre => pre.scrollWidth > pre.clientWidth + 1),
        code: [...inner.querySelectorAll('pre')].map(pre => pre.textContent),
      };
    })()`);
    assert.ok(Math.abs(layout.width - layout.available) <= 2, `Unused slide width at ${width}x900: ${JSON.stringify(layout)}`);
    assert.ok(layout.bottom <= layout.limit + 1, `Vertical overflow at ${width}x900`);
    assert.equal(layout.overflowingCode, false, `Code escaped its background at ${width}x900`);
    assert.deepEqual(layout.code, originalCode);
    if (width === 960 || width === 390) {
      const { data } = await cdp('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `code-${width}.png`), Buffer.from(data, 'base64'));
    }
  }
  await cdp('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await evaluate("location.hash = '/1'");
  await delay(350);
  const slideCount = await evaluate("document.querySelectorAll('section.slide').length");
  assert.equal(slideCount, 13);
  assert.equal(await evaluate("document.querySelector('mark').textContent"), 'Keep this takeaway in mind');
  await cdp('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowRight', code: 'ArrowRight' });
  await cdp('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowRight', code: 'ArrowRight' });
  await delay(350);
  assert.equal(await evaluate("document.querySelector('#cur').textContent"), '2');
  assert.equal(await evaluate('location.hash'), '#/2');
  await evaluate("document.querySelector('.slide.active a[href^=\"#\"]').click()");
  await delay(350);
  assert.equal(await evaluate("document.querySelector('#cur').textContent"), '11');
  assert.equal(await evaluate('location.hash'), '#latex');
  await cdp('Page.reload'); await ready();
  assert.equal(await evaluate("document.querySelector('#cur').textContent"), '11');
  assert.equal(await evaluate('location.hash'), '#latex');
  for (const [name, page] of [['controls', 2], ['headings', 3], ['text-style', 4], ['blockquotes', 5], ['alerts', 6], ['lists', 7], ['code', 8], ['table', 9], ['media', 10], ['formulas', 11], ['cover', 1], ['closing', 13], ['mermaid', 12]]) {
    await evaluate(`location.hash = '/${page}'`); await delay(350);
    const { data } = await cdp('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(output, `${name}.png`), Buffer.from(data, 'base64'));
  }
  await evaluate("document.querySelector('.slide.active .mermaid svg').dispatchEvent(new MouseEvent('click', { bubbles: true }))");
  await delay(100);
  assert.equal(await evaluate("document.querySelector('#diagram-modal').classList.contains('open')"), true);
  assert.equal(await evaluate("(() => { const ids = [...document.querySelectorAll('[id]')].map(n => n.id); return ids.length === new Set(ids).size; })()"), true);
  const initialDiagram = await evaluate("(() => { const r = document.querySelector('.diagram-stage').getBoundingClientRect(); return { x: r.x, y: r.y }; })()");
  const viewport = await evaluate("(() => { const r = document.querySelector('.diagram-viewport').getBoundingClientRect(); return { x: r.x + 150, y: r.y + 150 }; })()");
  await cdp('Input.dispatchMouseEvent', { type: 'mousePressed', x: viewport.x, y: viewport.y, button: 'left', buttons: 1, clickCount: 1 });
  await cdp('Input.dispatchMouseEvent', { type: 'mouseMoved', x: viewport.x + 120, y: viewport.y + 60, button: 'left', buttons: 1 });
  await cdp('Input.dispatchMouseEvent', { type: 'mouseReleased', x: viewport.x + 120, y: viewport.y + 60, button: 'left', buttons: 0, clickCount: 1 });
  const dragged = await evaluate("(() => { const r = document.querySelector('.diagram-stage').getBoundingClientRect(); const v = document.querySelector('.diagram-viewport'); return { x: r.x, y: r.y, scroll: v.scrollLeft + v.scrollTop, dragging: v.classList.contains('dragging') }; })()");
  assert.ok(Math.abs(dragged.x - initialDiagram.x - 120) < 1 && Math.abs(dragged.y - initialDiagram.y - 60) < 1, 'Dragging should pan the diagram');
  assert.equal(dragged.scroll, 0);
  assert.equal(dragged.dragging, false);
  await cdp('Input.dispatchMouseEvent', { type: 'mouseMoved', x: viewport.x + 150, y: viewport.y + 90 });
  assert.equal(await evaluate("document.querySelector('.diagram-stage').getBoundingClientRect().x"), dragged.x, 'Panning must stop on release');
  const beforeZoom = await evaluate("(() => { const r = document.querySelector('.diagram-stage').getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width }; })()");
  await cdp('Input.dispatchMouseEvent', { type: 'mouseWheel', x: viewport.x, y: viewport.y, deltaX: 0, deltaY: -100 });
  await delay(100);
  const afterZoom = await evaluate("(() => { const r = document.querySelector('.diagram-stage').getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width }; })()");
  const ratio = afterZoom.width / beforeZoom.width;
  assert.ok(ratio > 1, 'Wheel should zoom in');
  assert.ok(Math.abs(afterZoom.x - (viewport.x - (viewport.x - beforeZoom.x) * ratio)) < 1, 'Zoom should remain anchored to cursor');
  await evaluate("document.querySelector('[data-zoom=in]').click()");
  assert.ok(await evaluate("document.querySelector('.diagram-stage').getBoundingClientRect().width") > afterZoom.width);
  await evaluate("document.querySelector('[data-zoom=out]').click()");
  assert.ok(Math.abs(await evaluate("document.querySelector('.diagram-stage').getBoundingClientRect().width") - afterZoom.width) < 1);
  await evaluate("document.querySelector('[data-zoom=close]').click()");
  assert.equal(await evaluate("document.querySelector('#diagram-modal').classList.contains('open')"), false);
  await evaluate("document.querySelector('.slide.active .mermaid svg').dispatchEvent(new MouseEvent('click', { bubbles: true }))");
  await cdp('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape' });
  assert.equal(await evaluate("document.querySelector('#diagram-modal').classList.contains('open')"), false);
  for (const [width, height] of [[1440, 900], [900, 600], [390, 844]]) {
    await cdp('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
    for (let slide = 1; slide <= slideCount; slide++) {
      await evaluate(`location.hash = '/${slide}'`); await delay(330);
      const fits = await evaluate(`(() => {
        const slide = document.querySelector('.slide.active');
        const inner = slide.querySelector('.inner');
        const rect = inner.getBoundingClientRect();
        const scale = new DOMMatrix(getComputedStyle(inner).transform).a;
        return { right: rect.left + inner.scrollWidth * scale, bottom: rect.top + inner.scrollHeight * scale };
      })()`);
      assert.ok(fits.right <= width + 2 && fits.bottom <= height + 2, `${width}x${height}, slide ${slide}: ${JSON.stringify(fits)}`);
    }
  }
  assert.deepEqual(exceptions, []);
  const remoteImages = await evaluate("[...document.images].map(img => img.src).filter(src => /^https?:/.test(src))");
  assert.ok(requests.filter(url => /^https?:/.test(url)).every(url => remoteImages.includes(url)), 'Only explicitly linked remote images may use the network');
  await checkLayout(cdp, evaluate);
  // Exercise the actual browser reload client against a local preview process.
  liveDirectory = await mkdtemp(join(tmpdir(), 'slidown-live-'));
  const outline = join(liveDirectory, 'OUTLINE.md');
  const original = '# Live cover\n## First\nOriginal\n## Second\nKeep this page\n';
  await writeFile(outline, original);
  server = spawn(resolve('target/debug/slidown'), ['serve', '--port', '0', '--output', 'site'], {
    cwd: liveDirectory, stdio: ['ignore', 'pipe', 'pipe'],
  });
  const url = await new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error('Preview startup timed out')), 10000);
    server.on('error', reject);
    server.stdout.on('data', data => {
      output += data;
      const match = /Preview: (http:\/\/[^\s]+)/.exec(output);
      if (match) { clearTimeout(timer); resolve(match[1]); }
    });
    server.once('exit', code => { clearTimeout(timer); reject(new Error(`Preview exited ${code}`)); });
  });
  await cdp('Page.navigate', { url });
  await ready();
  await evaluate("location.hash = '/3'");
  await delay(350);
  async function eventually(expression) {
    for (let n = 0; n < 100; n++) {
      try { if (await evaluate(expression)) return; } catch (_) { /* document may be reloading */ }
      await delay(100);
    }
    throw new Error(`Timed out: ${expression}`);
  }
  await writeFile(outline, original.replace('Keep this page', 'Updated automatically'));
  await eventually("document.querySelector('.slide.active')?.textContent.includes('Updated automatically')");
  assert.equal(await evaluate("document.querySelector('#cur').textContent"), '3');
  await writeFile(outline, '# Live cover\nForbidden body\n## First');
  await eventually("[...document.querySelectorAll('[role=status]')].some(el => el.textContent.includes('only blank lines'))");
  assert.equal(await evaluate("document.querySelector('#cur').textContent"), '3');
  await writeFile(outline, original.replace('Keep this page', 'Recovered automatically'));
  await eventually("document.querySelector('.slide.active')?.textContent.includes('Recovered automatically')");
  assert.equal((await readFile(join(liveDirectory, 'site/index.html'), 'utf8')).includes('__slidown'), false);
  assert.deepEqual(exceptions, []);
  console.log('Browser checks passed: local assets, remote image URLs, text highlighting, navigation, anchors, SVG viewer drag/zoom/close, 3 viewport sizes, live reload, error recovery, current-page preservation.');
  console.log(`Screenshots: ${output}`);
} finally {
  socket?.close();
  if (server) {
    server.kill('SIGTERM');
    await new Promise(resolve => { if (server.exitCode !== null || server.signalCode !== null) resolve(); else server.once('exit', resolve); });
  }
  if (liveDirectory) await rm(liveDirectory, { recursive: true, force: true });
  browser.kill('SIGTERM');
  await new Promise(resolve => { if (browser.exitCode !== null || browser.signalCode !== null) resolve(); else browser.once('exit', resolve); });
  await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
