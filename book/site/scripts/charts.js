// Shared helpers for the figures on the home and "How it works" pages: DOM and SVG builders,
// scale bars, small SVG charts and the canvas file maps. Colours are read from the CSS tokens
// (tokens.css) at draw time, and every figure redraws when the theme changes.
export const $ = (s, el = document) => el.querySelector(s);
export const h = (tag, attrs = {}, html = "") => { const e = document.createElement(tag); for (const k in attrs) e.setAttribute(k, attrs[k]); if (html) e.innerHTML = html; return e; };
export const NS = "http://www.w3.org/2000/svg";
export const s = (tag, attrs = {}) => { const e = document.createElementNS(NS, tag); for (const k in attrs) e.setAttribute(k, attrs[k]); return e; };
export const fmtInt = n => n.toLocaleString("en-US");
export const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
export const css = (v, el = document.documentElement) => getComputedStyle(el).getPropertyValue(v).trim();
export const themeListeners = [];
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => themeListeners.forEach(f => f()));
new MutationObserver(() => themeListeners.forEach(f => f())).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

export function hexRows(hex, rows) {
  const out = [];
  for (let r = 0; r < rows && r * 32 < hex.length; r++) {
    const chunk = hex.slice(r * 32, r * 32 + 32);
    const bytes = chunk.match(/../g) || [];
    const ascii = bytes.map(b => { const c = parseInt(b, 16); return c >= 32 && c < 127 ? String.fromCharCode(c) : "."; }).join("");
    const off = (r * 16).toString(16).padStart(8, "0");
    out.push(`${off}  ${bytes.join(" ").padEnd(47)}  <b>${ascii.replace(/&/g,"&amp;").replace(/</g,"&lt;")}</b>`);
  }
  return out.join("\n");
}


/* ---------- scale bars ---------- */
export function scaleBar(el, umPerPx, targetPx) {
  const raw = targetPx * umPerPx, pow = Math.pow(10, Math.floor(Math.log10(raw)));
  const L = [5, 2, 1].map(k => k * pow).find(v => v <= raw * 1.15) || pow;
  const label = L < 1 ? `${+(L * 1000).toPrecision(3)} nm` : L >= 1000 ? `${+(L / 1000).toPrecision(3)} mm` : `${+L.toPrecision(3)} µm`;
  el.innerHTML = `<i style="width:${(L / umPerPx).toFixed(1)}px"></i><span>${label}</span>`;
}
export function coverBar(box, img, widthUm, cls) {
  const bar = h("div", { class: "scalebar " + cls, "aria-hidden": "true" }); box.append(bar);
  const upd = () => {
    const bw = box.clientWidth, bh = box.clientHeight, nw = img.naturalWidth, nh = img.naturalHeight;
    if (!bw || !nw) return;
    const k = Math.max(bw / nw, bh / nh);
    scaleBar(bar, widthUm / nw / k, bw * 0.22);
  };
  img.complete ? upd() : img.addEventListener("load", upd);
  if (window.ResizeObserver) new ResizeObserver(upd).observe(box);
}

/* ---------- charts ---------- */
export function nice(lo, hi, n = 4) {
  const span = hi - lo, step0 = span / n, mag = Math.pow(10, Math.floor(Math.log10(step0)));
  const step = [1, 2, 2.5, 5, 10].map(k => k * mag).find(k => span / k <= n) || mag * 10;
  const out = []; for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) out.push(+v.toFixed(10));
  return out;
}
export function frame(fig, o) {
  const W = 400, H = 300, L = 44, R = 14, T = 16, B = 34;
  const svg = s("svg", { class: "chart", viewBox: `0 0 ${W} ${H}`, role: "img", "aria-label": o.aria });
  const X = v => o.xrev ? L + (o.x1 - v) / (o.x1 - o.x0) * (W - L - R) : L + (v - o.x0) / (o.x1 - o.x0) * (W - L - R);
  const Y = v => H - B - (v - o.y0) / (o.y1 - o.y0) * (H - T - B);
  const ax = s("g", { class: "axis" });
  nice(o.y0, o.y1).forEach(v => { ax.append(s("line", { class: "gl", x1: L, x2: W - R, y1: Y(v), y2: Y(v) })); const t = s("text", { x: L - 6, y: Y(v) + 3, "text-anchor": "end" }); t.textContent = o.yf ? o.yf(v) : v; ax.append(t); });
  nice(o.x0, o.x1, 5).forEach(v => { const t = s("text", { x: X(v), y: H - B + 14, "text-anchor": "middle" }); t.textContent = o.xf ? o.xf(v) : v; ax.append(t); });
  const xl = s("text", { x: W - R, y: H - 6, "text-anchor": "end" }); xl.textContent = o.xlabel; ax.append(xl);
  const yl = s("text", { x: 6, y: 10 }); yl.textContent = o.ylabel; ax.append(yl);
  svg.append(ax); fig.append(svg);
  return { svg, X, Y, W, H, L, R, T, B };
}
export function hover(fig, c, xs, ys, fmt) {
  const line = s("line", { class: "cross", y1: c.T, y2: c.H - c.B, visibility: "hidden" });
  const dot = s("circle", { r: 4, fill: "var(--accent)", stroke: "var(--surface)", "stroke-width": 2, visibility: "hidden" });
  c.svg.append(line, dot);
  const tip = h("div", { class: "tip", hidden: "" }); fig.append(tip);
  c.svg.addEventListener("pointermove", e => {
    const r = c.svg.getBoundingClientRect(), vx = (e.clientX - r.left) / r.width * c.W;
    let best = 0, bd = Infinity;
    for (let i = 0; i < xs.length; i++) { const d = Math.abs(c.X(xs[i]) - vx); if (d < bd) { bd = d; best = i; } }
    const cx = c.X(xs[best]), cy = c.Y(ys[best]);
    line.setAttribute("x1", cx); line.setAttribute("x2", cx); dot.setAttribute("cx", cx); dot.setAttribute("cy", cy);
    line.setAttribute("visibility", "visible"); dot.setAttribute("visibility", "visible");
    tip.hidden = false; tip.textContent = fmt(xs[best], ys[best]);
    tip.style.left = (cx / c.W * r.width) + "px"; tip.style.top = (cy / c.H * r.height) + "px";
  });
  c.svg.addEventListener("pointerleave", () => { line.setAttribute("visibility", "hidden"); dot.setAttribute("visibility", "hidden"); tip.hidden = true; });
}
export function lineChart(fig, xs, ys, o) {
  const x0 = Math.min(...xs), x1 = Math.max(...xs);
  let y0 = Math.min(...ys), y1 = Math.max(...ys); const pad = (y1 - y0) * 0.06; y0 -= pad; y1 += pad;
  const c = frame(fig, { ...o, x0, x1, y0, y1 });
  let d = ""; xs.forEach((x, i) => d += (i ? "L" : "M") + c.X(x).toFixed(1) + " " + c.Y(ys[i]).toFixed(1));
  if (o.area) c.svg.append(s("path", { d: d + `L${c.X(xs[xs.length - 1])} ${c.Y(y0)}L${c.X(xs[0])} ${c.Y(y0)}Z`, fill: "var(--accent)", "fill-opacity": .12 }));
  c.svg.append(s("path", { d, fill: "none", stroke: "var(--accent)", "stroke-width": o.sw || 1.6, "stroke-linejoin": "round" }));
  hover(fig, c, xs, ys, o.tip);
  return c;
}
export const figs = {
  image(fig, cd) { fig.append(h("img", { src: cd.img, alt: cd.title, loading: "lazy" })); },
  trace(fig, cd) {
    lineChart(fig, cd.x, cd.y, { xlabel: "time (ms)", ylabel: "voltage (mV)", aria: "Voltage of a neuron over time, showing a train of spikes", sw: 1.1, tip: (x, y) => `${x.toFixed(1)} ms · ${y.toFixed(1)} mV` });
  },
  spectrum(fig, cd) {
    lineChart(fig, cd.x, cd.y, { xrev: true, xlabel: "wavenumber (cm⁻¹)", ylabel: "reflectance", aria: "Infrared spectrum of orange-peach juice", yf: v => v.toFixed(2), tip: (x, y) => `${x.toFixed(0)} cm⁻¹ · ${y.toFixed(3)}` });
  },
  chrom(fig, cd) {
    const ch = cd.chrom, ys = ch.intensity.map(v => v / 1e9);
    lineChart(fig, ch.rt_min, ys, { area: true, xlabel: "minutes", ylabel: "total signal (×10⁹)", aria: "Total ion chromatogram of a cannabis extract", yf: v => v.toFixed(1), tip: (x, y) => `${x.toFixed(2)} min · ${y.toFixed(2)}×10⁹` });
  },
  qpcr(fig, cd) {
    const n = cd.curves[0].y.length, all = cd.curves.flatMap(c => c.y);
    const c = frame(fig, { x0: 1, x1: n, y0: Math.min(...all) * .96, y1: Math.max(...all) * 1.03, xlabel: "cycle", ylabel: "glow (normalised)", aria: "Amplification curves of 90 PCR wells", yf: v => v.toFixed(1) });
    const cqs = cd.curves.map(k => k.cq), lo = Math.min(...cqs), hi = Math.max(...cqs);
    [...cd.curves].sort((a, b) => b.cq - a.cq).forEach(k => {
      let d = ""; k.y.forEach((v, i) => d += (i ? "L" : "M") + c.X(i + 1).toFixed(1) + " " + c.Y(v).toFixed(1));
      const t = (k.cq - lo) / (hi - lo);
      const p = s("path", { d, fill: "none", stroke: "var(--accent)", "stroke-opacity": (1 - t * 0.75).toFixed(2), "stroke-width": 1.2 });
      const ti = s("title"); ti.textContent = `well ${k.well}: rises at cycle ${k.cq}`; p.append(ti);
      c.svg.append(p);
    });
    const a = s("text", { x: c.L + 8, y: c.T + 12, fill: "var(--ink-2)", "font-size": 10, "font-family": "var(--sans)" }); a.textContent = "darker = more DNA, rises sooner"; c.svg.append(a);
  },
  plate(fig, cd) {
    const W = 400, H = 300, L = 26, T = 22, cols = 24, rowsN = 16, cell = Math.min((W - L - 10) / cols, (H - T - 34) / rowsN);
    const svg = s("svg", { class: "chart", viewBox: `0 0 ${W} ${H}`, role: "img", "aria-label": "Heat map of a 384-well plate" });
    const vals = cd.wells.map(w => w[2]), lo = Math.min(...vals), hi = Math.max(...vals);
    const ax = s("g", { class: "axis" });
    for (let r = 0; r < rowsN; r++) { const t = s("text", { x: L - 6, y: T + r * cell + cell * .7, "text-anchor": "end" }); t.textContent = String.fromCharCode(65 + r); ax.append(t); }
    for (let c = 0; c < cols; c += 1) if (c % 4 === 0 || c === 23) { const t = s("text", { x: L + c * cell + cell / 2, y: T - 6, "text-anchor": "middle" }); t.textContent = c + 1; ax.append(t); }
    svg.append(ax);
    const tip = h("div", { class: "tip", hidden: "" }); fig.append(tip);
    cd.wells.forEach(([r, c, v]) => {
      const t = Math.sqrt((v - lo) / (hi - lo));
      const rect = s("rect", { x: L + (c - 1) * cell + 1, y: T + (r - 1) * cell + 1, width: cell - 2, height: cell - 2, rx: 2, fill: "var(--accent)", "fill-opacity": (0.07 + t * 0.93).toFixed(3) });
      const name = String.fromCharCode(64 + r) + c;
      rect.addEventListener("pointerenter", () => { const b = svg.getBoundingClientRect(); tip.hidden = false; tip.textContent = `${name} · ${v.toFixed(3)}`; tip.style.left = ((L + (c - .5) * cell) / W * b.width) + "px"; tip.style.top = ((T + (r - 1) * cell) / H * b.height) + "px"; });
      rect.addEventListener("pointerleave", () => tip.hidden = true);
      svg.append(rect);
    });
    const ly = T + rowsN * cell + 14, lx = L, lw = 150;
    const g = s("defs"), lg = s("linearGradient", { id: "pg" });
    lg.append(s("stop", { offset: "0", "stop-color": "var(--accent)", "stop-opacity": .07 }), s("stop", { offset: "1", "stop-color": "var(--accent)", "stop-opacity": 1 }));
    g.append(lg); svg.append(g);
    svg.append(s("rect", { x: lx, y: ly, width: lw, height: 8, rx: 2, fill: "url(#pg)" }));
    const l1 = s("text", { x: lx + lw + 8, y: ly + 8, fill: "var(--muted)", "font-size": 9, "font-family": "var(--mono)" }); l1.textContent = `absorbance ${lo.toFixed(2)} → ${hi.toFixed(2)}`; svg.append(l1);
    fig.append(svg);
  },
  scatter(fig, cd) {
    const c = frame(fig, { x0: -0.1, x1: 0.8, y0: -0.1, y1: 1.0, xlabel: "CD4 (helper marker) →", ylabel: "CD8 (killer marker) ↑", aria: "Scatter plot of immune cells by CD4 and CD8", xf: () => "", yf: () => "" });
    let dO = "", dT = "";
    cd.pts.forEach(([x, y, z]) => { const seg = `M${c.X(x).toFixed(1)} ${c.Y(y).toFixed(1)}h0`; if (z > 0.45) dT += seg; else dO += seg; });
    c.svg.append(s("path", { d: dO, stroke: "var(--mark-gray-2)", "stroke-width": 2.2, "stroke-linecap": "round", fill: "none" }));
    c.svg.append(s("path", { d: dT, stroke: "var(--accent)", "stroke-opacity": .55, "stroke-width": 2.2, "stroke-linecap": "round", fill: "none" }));
    const lab = (x, y, t, anchor = "start") => { const e = s("text", { x: c.X(x), y: c.Y(y), fill: "var(--ink)", "font-size": 11, "font-weight": 600, "font-family": "var(--sans)", "text-anchor": anchor }); e.textContent = t; c.svg.append(e); };
    lab(0.02, 0.93, "killer T cells");
    lab(0.78, 0.5, "helper T cells", "end");
    const k = s("text", { x: c.W - c.R, y: c.T + 10, "text-anchor": "end", fill: "var(--muted)", "font-size": 9, "font-family": "var(--mono)" }); k.textContent = "blue: T cells · grey: other cells"; c.svg.append(k);
  },
};

export const fmtBytes = b => b >= 1e9 ? `${(b / 1e9).toFixed(2)} GB` : b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB` : b >= 1e3 ? `${Math.round(b / 1e3)} KB` : `${b} bytes`;
export const COLOR = el => ({ d: css("--d", el), m: css("--m", el), s: css("--s", el), o: css("--o", el), x: css("--x", el), panel: css("--panel", el), panel2: css("--panel-2", el), ink: css("--ink", el), muted: css("--muted", el), paper: css("--paper", el), rule: css("--rule", el) });
export function hiDPI(cv, w, hgt) {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  cv.width = Math.round(w * dpr); cv.height = Math.round(hgt * dpr); cv.style.height = hgt + "px";
  const ctx = cv.getContext("2d"); ctx.setTransform(dpr, 0, 0, dpr, 0, 0); return ctx;
}
export const LEVEL_ALPHA = { 1: 1, 2: .62, 4: .5, 8: .42, 16: .36, 32: .31, 64: .27, 128: .24, 256: .22 };
export const SIZE_NAME = { 1: "full resolution", 2: "half size", 4: "a quarter size", 8: "1/8 size", 16: "1/16 size", 32: "1/32 size", 64: "1/64 size", 128: "1/128 size", 256: "1/256 size" };
export function segText(sg) {
  if (sg.tile) return `<b>Tile at ${SIZE_NAME[sg.scale]}</b><br>${fmtInt(sg.w / sg.scale)} × ${fmtInt(sg.h / sg.scale)} pixels, JPEG XR<br>${fmtBytes(sg.size)} at byte ${fmtInt(sg.off)}`;
  const names = { "file-header": "Header: where the catalogue is", "metadata": "Description of the acquisition (XML)", "subblock-directory": "Catalogue of all 8,529 tiles", "attachment-directory": "List of attachments", "attachment": "Attachment", "deleted": "Leftover space from earlier edits" };
  return `<b>${names[sg.kind] || sg.kind}</b><br>${sg.name ? sg.name + "<br>" : ""}${fmtBytes(sg.size)} at byte ${fmtInt(sg.off)}`;
}
export function drawSegs(ctx, segs, x0, w, y, hgt, b0, b1, cols, opts = {}) {
  const span = b1 - b0, n = Math.max(1, Math.round(w));
  ctx.fillStyle = cols.panel2; ctx.fillRect(x0, y, w, hgt);
  for (let k = 0; k < n; k++) {
    const b = b0 + (k + .5) / n * span, sg = segAt(segs, b);
    if (!sg || b >= sg.off + sg.size) continue;
    let alpha = sg.tile ? LEVEL_ALPHA[sg.scale] : 1;
    if (opts.dim && !opts.dim(sg)) alpha *= .14;
    ctx.globalAlpha = alpha; ctx.fillStyle = cols[sg.c]; ctx.fillRect(x0 + k * w / n, y, w / n + .5, hgt);
  }
  if (opts.min) for (const sg of segs) {
    if (sg.tile || sg.off + sg.size < b0 || sg.off > b1) continue;
    const pw = sg.size / span * w; if (pw >= opts.min) continue;
    ctx.globalAlpha = 1; ctx.fillStyle = cols[sg.c]; ctx.fillRect(x0 + (sg.off - b0) / span * w, y, opts.min, hgt);
  }
  ctx.globalAlpha = 1;
}
export function segAt(segs, b) {
  let lo = 0, hi = segs.length - 1, best = null;
  while (lo <= hi) { const mid = (lo + hi) >> 1; if (segs[mid].off <= b) { best = mid; lo = mid + 1; } else hi = mid - 1; }
  if (best === null) return null;
  for (let k = best; k >= Math.max(0, best - 40); k--) { const sg = segs[k]; if (sg.off <= b && b < sg.off + sg.size) return sg; }
  return segs[best];
}
export function tipAt(tip, box, x, y, html) { tip.hidden = false; tip.innerHTML = html; tip.style.left = Math.max(120, Math.min(box.clientWidth - 120, x)) + "px"; tip.style.top = y + "px"; }
