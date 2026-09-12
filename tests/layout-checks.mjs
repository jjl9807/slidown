// Boundary checks using generated slides, without changing the public example.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

export async function checkLayout(cdp, evaluate) {
  const directory = await mkdtemp(join(tmpdir(), 'slidown-layout-'));
  try {
    await cdp('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    const svg = (width, height) => `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}"><rect width="100%" height="100%" fill="#dbeafe"/></svg>`;
    await writeFile(join(directory, 'tall.svg'), svg(200, 10000));
    await writeFile(join(directory, 'wide.svg'), svg(10000, 200));
    const columns = Array.from({ length: 12 }, (_, i) => `Column ${i + 1}`);
    const source = [
      '---',
      `author: "${'Long author name '.repeat(8)}"`,
      `email: "${'long-address-'.repeat(8)}@example.com"`,
      'date: "2026-09-12"',
      `closing: "${'A long closing title '.repeat(10)}"`,
      '---',
      `# ${'LongCoverTitle'.repeat(16)}`,
      `## ${'A long slide title '.repeat(16)}`,
      '### A heading within the slide',
      ...Array(8).fill('A paragraph with **emphasis**, `inline code`, and ordinary words. '.repeat(6)),
      '## Unbroken code',
      '```text', 'long_identifier_' + 'x'.repeat(4000), '```',
      '## Wide table',
      `| ${columns.join(' | ')} |`,
      `| ${columns.map(() => '---').join(' | ')} |`,
      ...Array(5).fill(`| ${columns.map(() => 'longcell'.repeat(8)).join(' | ')} |`),
      '## Wide math',
      '$$', `\\frac{${Array(70).fill('a+b').join('+')}}{1+x}`, '$$',
      '## Tall image',
      'A tall image with a short caption should leave the text readable.',
      '![Tall image](tall.svg)',
      '## Multiple images',
      'Images share the slide with this caption.',
      '![Tall](tall.svg)', '![Wide](wide.svg)', '![Tall again](tall.svg)',
      '## Nested content',
      '> A quote with a list:', '>', '> - An item', '>   - A nested item',
      '- [x] A completed task with enough text to wrap in a narrow window.',
      '  - [ ] A nested task with an inline formula $E=mc^2$.',
      '```mermaid', 'flowchart TB', 'A[Start] --> B[Finish]', '```',
    ].join('\n');
    await writeFile(join(directory, 'OUTLINE.md'), source);
    execFileSync(resolve('target/debug/slidown'), ['build'], { cwd: directory, stdio: 'pipe' });
    await cdp('Page.navigate', { url: pathToFileURL(join(directory, 'dist/index.html')).href });
    await evaluate('document.fonts.ready');
    await delay(200);
    const count = await evaluate("document.querySelectorAll('section.slide').length");
    assert.equal(count, 9);
    const metrics = `(() => {
      const slide = document.querySelector('.slide.active');
      const inner = slide.querySelector('.inner');
      const title = slide.querySelector('.titlebar');
      const css = getComputedStyle(slide);
      const left = parseFloat(css.paddingLeft), right = slide.clientWidth - parseFloat(css.paddingRight);
      const bottom = slide.clientHeight - parseFloat(css.paddingBottom);
      const scale = new DOMMatrix(getComputedStyle(inner).transform).a;
      const rect = inner.getBoundingClientRect();
      const titleRect = title.getBoundingClientRect();
      const visible = getComputedStyle(inner).display !== 'none';
      const boxes = [...inner.querySelectorAll('svg, img, pre, table, input')].map(el => el.getBoundingClientRect());
      return { scale, bodyFits: !visible || (rect.left >= left - 1 && rect.right <= right + 1 && rect.bottom <= bottom + 1),
        titleFits: titleRect.left >= -1 && titleRect.right <= slide.clientWidth + 1 && titleRect.top >= -1 && titleRect.bottom <= slide.clientHeight + 1,
        contentFits: !visible || boxes.every(box => box.left >= left - 1 && box.right <= right + 1 && box.bottom <= bottom + 1),
        body: rect.toJSON(), title: titleRect.toJSON() };
    })()`;
    for (const [width, height] of [[1440, 900], [960, 900], [900, 240], [320, 720], [240, 180], [1440, 80], [1440, 900]]) {
      await cdp('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
      for (let page = 1; page <= count; page++) {
        await evaluate(`location.hash = '/${page}'`);
        await delay(330);
        const result = await evaluate(metrics);
        assert.ok(Number.isFinite(result.scale) && result.scale > 0 && result.scale <= 1, JSON.stringify(result));
        assert.ok(result.bodyFits && result.titleFits && result.contentFits, `${width}x${height}, page ${page}: ${JSON.stringify(result)}`);
        if (width === 1440 && height === 900 && (page === 6 || page === 7)) {
          assert.ok(result.scale > .5, `Images unnecessarily shrank the captions: ${JSON.stringify(result)}`);
        }
      }
    }
    // Repeated layout must converge instead of creating a ResizeObserver feedback loop.
    await evaluate(`(() => {
      const original = fitSlides;
      window.layoutCalls = 0;
      fitSlides = () => { window.layoutCalls++; original(); };
      location.hash = '/6';
    })()`);
    await delay(500);
    const calls = await evaluate('window.layoutCalls');
    const before = await evaluate(metrics);
    await delay(500);
    assert.equal(await evaluate('window.layoutCalls'), calls, 'Layout kept running after the page settled');
    assert.deepEqual(await evaluate(metrics), before, 'Layout drifted while the viewport was unchanged');

    // An image finishing later must also trigger a fit, without a resize event.
    await evaluate(`(() => {
      const img = document.querySelector('.slide.active img');
      img.src = 'data:image/svg+xml,' + encodeURIComponent(${JSON.stringify(svg(200, 20000))});
    })()`);
    await evaluate("Promise.all([...document.querySelectorAll('.slide.active img')].map(img => img.decode()))");
    await delay(200);
    const loaded = await evaluate(metrics);
    assert.ok(loaded.bodyFits && loaded.contentFits && loaded.scale > .5, JSON.stringify(loaded));

    // A diagram's initial fit and zoom floor must accommodate extreme aspect ratios.
    await evaluate("location.hash = '/8'");
    await delay(350);
    await evaluate(`(() => {
      const svg = document.querySelector('.slide.active .mermaid svg');
      svg.setAttribute('width', '100'); svg.setAttribute('height', '10000');
      svg.setAttribute('viewBox', '0 0 100 10000');
      svg.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    })()`);
    await delay(200);
    const diagramMetrics = `(() => {
      const viewport = document.querySelector('.diagram-viewport');
      const bounds = viewport.getBoundingClientRect();
      const diagram = document.querySelector('.diagram-stage svg').getBoundingClientRect();
      return { width: diagram.width, height: diagram.height,
        fits: diagram.left >= bounds.left - 1 && diagram.right <= bounds.right + 1 && diagram.top >= bounds.top - 1 && diagram.bottom <= bounds.bottom + 1 };
    })()`;
    const fitted = await evaluate(diagramMetrics);
    assert.ok(fitted.fits, `Tall diagram did not fit on opening: ${JSON.stringify(fitted)}`);
    await evaluate("document.querySelector('[data-zoom=in]').click()");
    const enlarged = await evaluate(diagramMetrics);
    assert.ok(Math.abs(enlarged.height / fitted.height - 1.25) < .01, 'Zoom jumped from a small initial fit');
    await evaluate("document.querySelector('[data-zoom=out]').click()");
    assert.ok((await evaluate(diagramMetrics)).fits);
    await cdp('Emulation.setDeviceMetricsOverride', { width: 900, height: 240, deviceScaleFactor: 1, mobile: false });
    await delay(200);
    assert.ok((await evaluate(diagramMetrics)).fits, 'Tall diagram did not refit after resizing');
    console.log('Layout boundary checks passed: long titles, code, tables, formulas, images, nested content, short viewports, stability, late images, and tall diagram zoom.');
  } finally {
    await cdp('Emulation.setEmulatedMedia', { features: [] });
    await cdp('Page.navigate', { url: 'about:blank' });
    await rm(directory, { recursive: true, force: true, maxRetries: 5 });
  }
}
