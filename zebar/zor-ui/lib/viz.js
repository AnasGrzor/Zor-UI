// Shared bar drawing for the desktop and bar visualizers.
const css = name => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#e5b6f2';

export function drawBars(canvas, levels, peaks, { style = 'desktop' } = {}) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth, h = canvas.clientHeight;
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
  }
  const ctx = canvas.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const n = levels.length;
  if (!n) return;
  const primary = css('--primary');
  const gap = style === 'bar' ? 1.5 : Math.max(2, w / n * 0.22);
  const bw = Math.max(1, w / n - gap);
  const grad = ctx.createLinearGradient(0, h, 0, 0);
  grad.addColorStop(0, primary);
  grad.addColorStop(1, style === 'bar' ? primary : primary + '66');
  ctx.fillStyle = grad;
  for (let i = 0; i < n; i++) {
    const v = Math.max(0, Math.min(1, levels[i] || 0));
    const bh = Math.max(style === 'bar' ? 2 : 3, v * h * (style === 'bar' ? 1 : 0.92));
    const x = i * (bw + gap) + gap / 2;
    const r = Math.min(bw / 2, 4);
    ctx.beginPath();
    if (ctx.roundRect) ctx.roundRect(x, h - bh, bw, bh, [r, r, 0, 0]);
    else ctx.rect(x, h - bh, bw, bh);
    ctx.fill();
    if (style !== 'bar' && peaks) {      // falling peak caps, like end4's cava widget
      peaks[i] = Math.max(v, (peaks[i] || 0) - 0.008);
      const py = h - peaks[i] * h * 0.92 - 6;
      ctx.fillStyle = primary;
      ctx.fillRect(x, py, bw, 2);
      ctx.fillStyle = grad;
    }
  }
}
