(() => {
  const banner = document.createElement('pre');
  banner.setAttribute('role', 'status');
  Object.assign(banner.style, {
    position: 'fixed', left: '1rem', right: '1rem', bottom: '1rem', zIndex: '100',
    padding: '1rem', background: '#fff8c5', color: '#24292f', border: '1px solid #9a6700',
    whiteSpace: 'pre-wrap', overflow: 'auto', maxHeight: '40vh', fontSize: '14px', display: 'none',
  });
  document.body.append(banner);
  async function poll() {
    try {
      const response = await fetch('/__slidown/status', { cache: 'no-store' });
      if (!response.ok) throw new Error('Preview disconnected');
      const status = await response.json();
      if (!status.error && status.revision !== window.__slidownRevision) {
        location.reload();
        return;
      }
      banner.textContent = status.error || status.warnings.join('\n');
      banner.style.display = banner.textContent ? 'block' : 'none';
    } catch (_) {
      banner.textContent = '预览服务已断开，正在尝试重新连接……';
      banner.style.display = 'block';
    }
    setTimeout(poll, 500);
  }
  poll();
})();
