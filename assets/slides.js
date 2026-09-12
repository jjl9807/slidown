const slides = Array.from(document.querySelectorAll('section.slide'));
const diagramModal = document.getElementById('diagram-modal');
const diagramStage = diagramModal.querySelector('.diagram-stage');
const diagramViewport = diagramModal.querySelector('.diagram-viewport');
let diagramZoom = 1;
let diagramMinZoom = .1;
let diagramPan = { x: 0, y: 0 };
let diagramDrag;
let diagramPreviousFocus;
function updateDiagramZoom() {
  diagramStage.style.transform = `translate(${diagramPan.x}px, ${diagramPan.y}px) scale(${diagramZoom})`;
}
function diagramBounds() {
  const css = getComputedStyle(diagramViewport);
  const rect = diagramViewport.getBoundingClientRect();
  const left = parseFloat(css.paddingLeft);
  const top = parseFloat(css.paddingTop);
  return {
    x: rect.left + left,
    y: rect.top + top,
    width: Math.max(1, diagramViewport.clientWidth - left - parseFloat(css.paddingRight)),
    height: Math.max(1, diagramViewport.clientHeight - top - parseFloat(css.paddingBottom)),
  };
}
function centerDiagram() {
  const diagram = diagramStage.querySelector('svg, img');
  if (!diagram) return;
  const bounds = diagramBounds();
  const rect = diagram.getBoundingClientRect();
  diagramPan = { x: (bounds.width - rect.width) / 2, y: (bounds.height - rect.height) / 2 };
  updateDiagramZoom();
}
function zoomDiagram(zoom, point) {
  const bounds = diagramBounds();
  const anchor = point ? { x: point.x - bounds.x, y: point.y - bounds.y }
    : { x: bounds.width / 2, y: bounds.height / 2 };
  const next = Math.max(diagramMinZoom, Math.min(5, zoom));
  const ratio = next / diagramZoom;
  // Keep the point under the mouse (or viewport center) fixed while zooming.
  diagramPan = {
    x: anchor.x - (anchor.x - diagramPan.x) * ratio,
    y: anchor.y - (anchor.y - diagramPan.y) * ratio,
  };
  diagramZoom = next;
  updateDiagramZoom();
}
function fitDiagramToViewport() {
  const diagram = diagramStage.querySelector('svg, img');
  if (!diagram || !diagramModal.classList.contains('open')) return;
  endDiagramDrag();
  diagramZoom = 1;
  diagramPan = { x: 0, y: 0 };
  updateDiagramZoom();
  const bounds = diagramBounds();
  const rect = diagram.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  diagramZoom = Math.min(1, bounds.width / rect.width, bounds.height / rect.height);
  diagramMinZoom = Math.min(.1, diagramZoom);
  updateDiagramZoom();
  centerDiagram();
}
function endDiagramDrag() {
  const drag = diagramDrag;
  diagramDrag = undefined;
  diagramViewport.classList.remove('dragging');
  if (drag && diagramViewport.hasPointerCapture(drag.id)) diagramViewport.releasePointerCapture(drag.id);
}
function closeDiagramModal() {
  endDiagramDrag();
  diagramModal.classList.remove('open');
  diagramModal.setAttribute('aria-hidden', 'true');
  diagramStage.replaceChildren();
  diagramZoom = 1;
  diagramPan = { x: 0, y: 0 };
  updateDiagramZoom();
  diagramPreviousFocus?.focus({ preventScroll: true });
}
document.addEventListener('click', event => {
  const diagram = event.target.closest('.slide.active .mermaid :is(svg, img), .slide.active :is(svg, img).mermaid');
  if (diagram) {
    event.preventDefault();
    diagramPreviousFocus = document.activeElement;
    const copy = diagram.cloneNode(true);
    // Modal copies need their own fragment IDs, including marker and clip references.
    const elements = [copy, ...copy.querySelectorAll('*')];
    const ids = new Map(elements.filter(el => el.id).map(el => [el.id, `modal-${el.id}`]));
    for (const el of elements) {
      for (const attr of [...el.attributes]) {
        let value = attr.value;
        if (attr.name === 'id') value = ids.get(value) || value;
        else if ((attr.name === 'href' || attr.name === 'xlink:href') && value.startsWith('#')) {
          value = '#' + (ids.get(value.slice(1)) || value.slice(1));
        } else value = value.replace(/url\(#([^)]+)\)/g, (_, id) => `url(#${ids.get(id) || id})`);
        if (value !== attr.value) el.setAttribute(attr.name, value);
      }
    }
    copy.addEventListener('load', fitDiagramToViewport, { once: true });
    diagramStage.replaceChildren(copy);
    diagramZoom = 1;
    updateDiagramZoom();
    diagramModal.classList.add('open');
    requestAnimationFrame(fitDiagramToViewport);
    diagramModal.setAttribute('aria-hidden', 'false');
    diagramViewport.focus({ preventScroll: true });
  } else if (event.target === diagramModal) closeDiagramModal();
});
diagramModal.querySelectorAll('[data-zoom]').forEach(button => button.addEventListener('click', event => {
  event.stopPropagation();
  const action = button.dataset.zoom;
  if (action === 'close') return closeDiagramModal();
  endDiagramDrag();
  if (action === 'in') zoomDiagram(diagramZoom * 1.25);
  if (action === 'out') zoomDiagram(diagramZoom / 1.25);
}));
diagramViewport.addEventListener('pointerdown', event => {
  if (!diagramModal.classList.contains('open') || event.button !== 0 || !event.isPrimary) return;
  event.preventDefault();
  diagramDrag = { id: event.pointerId, x: event.clientX, y: event.clientY, pan: { ...diagramPan } };
  diagramViewport.setPointerCapture(event.pointerId);
  diagramViewport.classList.add('dragging');
});
diagramViewport.addEventListener('pointermove', event => {
  if (!diagramDrag || event.pointerId !== diagramDrag.id) return;
  diagramPan = {
    x: diagramDrag.pan.x + event.clientX - diagramDrag.x,
    y: diagramDrag.pan.y + event.clientY - diagramDrag.y,
  };
  updateDiagramZoom();
});
for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) {
  diagramViewport.addEventListener(name, event => {
    if (diagramDrag?.id === event.pointerId) endDiagramDrag();
  });
}
diagramViewport.addEventListener('dragstart', event => event.preventDefault());
diagramViewport.addEventListener('click', event => event.preventDefault());
diagramViewport.addEventListener('wheel', event => {
  if (!diagramModal.classList.contains('open')) return;
  event.preventDefault();
  endDiagramDrag();
  zoomDiagram(diagramZoom * (event.deltaY < 0 ? 1.1 : .9), { x: event.clientX, y: event.clientY });
}, { passive: false });
window.addEventListener('resize', fitDiagramToViewport);

let i = 0;
const cur = document.getElementById('cur');
const total = document.getElementById('total');

total.textContent = slides.length;

// Initialize the active slide and its layout.
fitSlides();

function show(n, updateHash = true) {
  const target = Math.max(0, Math.min(n, slides.length - 1));
  if (target === i) return;                       // Stay on the current slide at either boundary.
  const dir = target > i ? 1 : -1;                // Enter from the right when advancing, or the left when going back.
  const old = slides[i];
  slides.forEach(s => s.classList.remove('leaving', 'no-anim'));
  old.style.setProperty('--dir', dir);
  old.classList.remove('active');
  old.classList.add('leaving');                   // Keep the outgoing slide underneath until the transition ends.
  old.addEventListener('animationend', () => old.classList.remove('leaving'), { once: true });
  i = target;
  slides[i].style.setProperty('--dir', dir);
  slides[i].classList.add('active');
  cur.textContent = i + 1;
  window.scrollTo(0, 0);
  fitSlides();
  if (updateHash) history.replaceState(null, '', `#/${i + 1}`);
}

// Size the title independently, then fit the body without narrowing its visible width.
function fitSlides() {
  slides.forEach(s => {
    if (!s.classList.contains('active')) return; // Hidden slides have no measurable dimensions.
    const inner = s.querySelector('.inner');
    const title = s.querySelector('.titlebar');
    const css = getComputedStyle(s);
    const width = s.clientWidth - parseFloat(css.paddingLeft) - parseFloat(css.paddingRight);
    const bottom = s.clientHeight - parseFloat(css.paddingBottom);
    // Cap title height at 30% of the viewport to leave room for content in small windows or with long titles.
    if (title) {
      title.style.fontSize = '';
      const base = parseFloat(css.fontSize);
      let lo = 0, hi = base;
      for (let n = 0; n < 16; n++) {
        const size = (lo + hi) / 2;
        title.style.fontSize = `${size}px`;
        if (title.offsetHeight <= s.clientHeight * .3 && title.scrollWidth <= title.clientWidth) lo = size;
        else hi = size;
      }
      title.style.fontSize = `${lo}px`;
    }
    inner.style.transform = 'none';
    const height = Math.max(1, bottom - inner.offsetTop);
    const imageCount = Math.max(1, inner.querySelectorAll('img').length);
    const availableWidth = Math.max(1, width - 1);
    const availableHeight = Math.max(1, height - 1);
    // Keep image caps in layout pixels so images shrink along with captions and margins.
    inner.style.setProperty('--image-height', `${availableHeight / imageCount}px`);
    function layout(scale) {
      // Compensate for the transform before measuring wraps and intrinsic code widths.
      inner.style.width = `${availableWidth / scale}px`;
      return inner.scrollWidth * scale <= availableWidth + .5
        && Math.max(inner.scrollHeight, inner.offsetHeight) * scale <= availableHeight + .5;
    }
    let scale = 1;
    if (!layout(scale)) {
      // Find a fitting lower bound even for exceptionally long unbroken code lines.
      let hi = 1;
      for (let n = 0; n < 32; n++) {
        scale /= 2;
        if (layout(scale)) break;
        hi = scale;
      }
      let lo = scale;
      for (let n = 0; n < 16; n++) {
        const candidate = (lo + hi) / 2;
        if (layout(candidate)) lo = candidate;
        else hi = candidate;
      }
      scale = lo;
      layout(scale);
    }
    inner.style.transform = `scale(${scale})`;
  });
}
// Coalesce asynchronous layout events; transforms leave layout dimensions unchanged, avoiding observer loops.
let fitFrame;
function scheduleFit() {
  cancelAnimationFrame(fitFrame);
  fitFrame = requestAnimationFrame(fitSlides);
}
slides.forEach(s => {
  if (window.ResizeObserver) {
    const observer = new ResizeObserver(scheduleFit);
    observer.observe(s.querySelector('.inner'));
    const title = s.querySelector('.titlebar');
    if (title) observer.observe(title);
  }
  s.addEventListener('load', scheduleFit, true);
  s.addEventListener('error', scheduleFit, true);
});
window.addEventListener('resize', scheduleFit);
window.addEventListener('load', scheduleFit);
if (document.fonts) {
  document.fonts.ready.then(scheduleFit);
  document.fonts.addEventListener('loadingdone', scheduleFit);
}

document.addEventListener('keydown', (e) => {
  if (diagramModal.classList.contains('open')) {
    if (e.key === 'Escape') {
      e.preventDefault();
      closeDiagramModal();
    }
    return;
  }
  if (e.target.closest('input, textarea, select, button, a, [contenteditable="true"]')) return;
  switch (e.key) {
    case ' ': case 'ArrowRight': case 'ArrowDown': case 'j': case 'l': case 'Enter': case 'PageDown':
      e.preventDefault(); show(i + 1); break;
    case 'ArrowLeft': case 'ArrowUp': case 'h': case 'k': case 'Backspace': case 'PageUp':
      e.preventDefault(); show(i - 1); break;
    case 'Home': e.preventDefault(); show(0); break;
    case 'End': e.preventDefault(); show(slides.length - 1); break;
    case 'f':
      if (!document.fullscreenElement) document.documentElement.requestFullscreen?.().catch(() => {});
      else document.exitFullscreen?.().catch(() => {});
      break;
    case 'q':
      window.close();
      break;
  }
});

// Hashes preserve the current page across reloads and reveal linked headings.
function followHash() {
  let hash;
  try { hash = decodeURIComponent(location.hash.slice(1)); } catch (_) { return; }
  const numeric = /^\/(\d+)$/.exec(hash);
  if (numeric) { show(Number(numeric[1]) - 1); return; }
  const target = document.getElementById(hash)?.closest('section.slide');
  if (target) show(slides.indexOf(target), false);
}
window.addEventListener('hashchange', followHash);
followHash();
