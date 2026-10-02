// Figures of "How it works". Data: site/data/how.json (the Zeiss slide scan, nine files from
// nine instruments, and the index of the test collection).
import HOW from "../data/how.json";
import { $, h, fmtInt, fmtBytes, COLOR, hiDPI, LEVEL_ALPHA, SIZE_NAME, segText, drawSegs, segAt, tipAt, scaleBar, themeListeners } from "./charts.js";
const BASE = import.meta.env.BASE_URL.replace(/\/?$/, "/");
const D = { mouse: HOW.mouse }, MAPS = HOW.maps, LAB = HOW.lab;

const MOUSE = (() => {
  const m = MAPS.mouse, segs = [];
  m.tiles.forEach(([off, size, scale, x, y, w, h], i) => segs.push({ off, size, c: "d", tile: true, scale, x, y, w, h, i }));
  m.other.forEach(([off, size, c, kind, name]) => segs.push({ off, size, c, kind, name }));
  segs.sort((a, b) => a.off - b.off);
  return { total: m.total, segs };
})();
(function fileMap() {
  const cv = $("#mapCv"), fig = $("#f-map"), tip = $("#mapTip");
  const T = MOUSE.total, IN1 = [0, 800e3], IN2 = [T - 12.5e6, T];
  let geo;
  function draw() {
    const W = cv.clientWidth || fig.clientWidth - 36, Hh = 230, ctx = hiDPI(cv, W, Hh), cols = COLOR(fig);
    const my = 30, mh = 46, iy = 150, ih = 34, gap = 24, iw = (W - gap) / 2;
    geo = { W, my, mh, iy, ih, iw, gap };
    ctx.font = "11px 'IBM Plex Sans', sans-serif"; ctx.textBaseline = "alphabetic";
    // scale ticks
    ctx.fillStyle = cols.muted; ctx.strokeStyle = cols.rule;
    for (let g = 0; g <= 3.5; g += .5) { const x = g * 1e9 / T * W; ctx.fillRect(x, my - 6, 1, 5); if (x > W - 70) continue; ctx.textAlign = g === 0 ? "left" : "center"; ctx.fillText(g === 0 ? "byte 0" : `${g} GB`, x, my - 10); }
    ctx.textAlign = "right"; ctx.fillText(`${(T / 1e9).toFixed(2)} GB`, W, my - 10);
    drawSegs(ctx, MOUSE.segs, 0, W, my, mh, 0, T, cols);
    // connectors
    ctx.fillStyle = cols.ink; ctx.globalAlpha = .06;
    const trap = (x0m, x1m, xa, xb) => { ctx.beginPath(); ctx.moveTo(x0m, my + mh); ctx.lineTo(x1m, my + mh); ctx.lineTo(xb, iy); ctx.lineTo(xa, iy); ctx.closePath(); ctx.fill(); };
    trap(0, 3, 0, iw); trap(W - 3, W, iw + gap, W); ctx.globalAlpha = 1;
    drawSegs(ctx, MOUSE.segs, 0, iw, iy, ih, IN1[0], IN1[1], cols, { min: 1.5 });
    drawSegs(ctx, MOUSE.segs, iw + gap, iw, iy, ih, IN2[0], IN2[1], cols, { min: 1.5 });
    ctx.fillStyle = cols.muted; ctx.textAlign = "left"; ctx.fillText("the first 800 KB", 0, iy + ih + 16);
    ctx.textAlign = "right"; ctx.fillText("the last 12.5 MB", W, iy + ih + 16);
    // annotations
    const ann = (seg, inset, text, side) => {
      const [b0, b1] = inset ? IN2 : IN1, xo = inset ? iw + gap : 0;
      const x = xo + ((seg.off + seg.size / 2) - b0) / (b1 - b0) * iw;
      ctx.strokeStyle = cols.ink; ctx.globalAlpha = .5; ctx.beginPath(); ctx.moveTo(x, iy - 2); ctx.lineTo(x, iy - 14); ctx.stroke(); ctx.globalAlpha = 1;
      ctx.fillStyle = cols.ink; const tw = ctx.measureText(text).width; let tx = side === "left" ? x - 2 : side === "right" ? x - tw + 2 : x - tw / 2; tx = Math.max(0, Math.min(W - tw, tx)); ctx.textAlign = "left"; ctx.fillText(text, tx, iy - 18);
    };
    const find = k => MOUSE.segs.find(s => s.kind === k);
    ann(find("file-header"), 0, "header", "left");
    ann(find("metadata"), 0, "description", "center");
    const firstTile = MOUSE.segs.find(s => s.tile);
    ann({ off: firstTile.off, size: 0 }, 0, "tiles begin", "left");
    ann(find("subblock-directory"), 1, "catalogue of 8,529 tiles", "center");
    const att = MOUSE.segs.filter(s => s.kind === "attachment").pop();
    if (att) ann(att, 1, "attachments", "right");
  }
  function hit(e) {
    const r = cv.getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top, g = geo;
    let b = null;
    if (y >= g.my && y <= g.my + g.mh) b = x / g.W * T;
    else if (y >= g.iy && y <= g.iy + g.ih) b = x < g.iw ? IN1[0] + x / g.iw * (IN1[1] - IN1[0]) : x > g.iw + g.gap ? IN2[0] + (x - g.iw - g.gap) / g.iw * (IN2[1] - IN2[0]) : null;
    if (b === null) { tip.hidden = true; return; }
    const sg = segAt(MOUSE.segs, b); if (!sg) { tip.hidden = true; return; }
    tipAt(tip, fig, x + 18, y + 18, segText(sg));
  }
  cv.addEventListener("pointermove", hit); cv.addEventListener("pointerleave", () => tip.hidden = true);
  const leg = h("div", { class: "map-legend" }, `<span class="k d">tiles of pixels</span><span class="k m">description</span><span class="k s">signposts</span><span class="k o">leftover space</span>`);
  fig.append(leg);
  draw(); addEventListener("resize", draw); themeListeners.push(draw);
})();

/* ---------- figure: reading only what's needed ---------- */
(function zoomRead() {
  const st = MAPS.mouse.steps, m = D.mouse, cv = $("#zrMap"), fig = $("#f-zoom");
  const labels = ["whole mouse", "near the snout", "closer", "single cells"];
  const imgs = [m.overview, ...m.zooms.map(z => z.uri)].map(u => BASE + u);
  let cur = 0;
  function show(i) {
    cur = i; const s = st[i], box = $("#zrImg");
    box.innerHTML = ""; box.append(h("img", { src: imgs[i], alt: `The mouse slide, ${labels[i]}` }));
    const bar = h("div", { class: "scalebar on-light" }); box.append(bar);
    const im = box.querySelector("img"), upd = () => { const bw = box.clientWidth, bh = box.clientHeight, nw = im.naturalWidth, nh = im.naturalHeight; if (!nw) return; const k = Math.max(bw / nw, bh / nh); scaleBar(bar, s.r[2] * 0.22 / nw / k, bw * .2); };
    im.complete ? upd() : im.addEventListener("load", upd);
    const read = s.bytes + MAPS.mouse.sig;
    $("#zrWhat").innerHTML = `Showing <b>${labels[i]}</b>: ${fmtInt(s.r[2])} × ${fmtInt(s.r[3])} pixels of the original, drawn at ${SIZE_NAME[s.scale]}.`;
    $("#zrBig").innerHTML = `${(read / 1e6).toFixed(1)} MB <small>read of ${fmtInt(Math.round(MOUSE.total / 1e6))} MB</small>`;
    $("#zrMeter").style.width = Math.max(read / MOUSE.total * 100, .3) + "%";
    $("#zrDetail").textContent = `${s.tiles.length} tile${s.tiles.length > 1 ? "s" : ""} (${fmtBytes(s.bytes)}) plus the header, description and catalogue (${fmtBytes(MAPS.mouse.sig)}). The other ${fmtInt(MOUSE.segs.length - s.tiles.length - 4)} records are never touched.`;
    [...$("#zrSteps").children].forEach((b, k) => b.setAttribute("aria-pressed", String(k === i)));
    drawMap();
  }
  function drawMap() {
    const s = st[cur], W = cv.clientWidth || fig.clientWidth - 36, ctx = hiDPI(cv, W, 46), cols = COLOR(fig);
    const want = new Set(s.tiles);
    drawSegs(ctx, MOUSE.segs, 0, W, 14, 26, 0, MOUSE.total, cols, { dim: sg => sg.tile ? want.has(sg.i) : sg.c === "s" || sg.c === "m" });
    ctx.fillStyle = cols.d;
    MOUSE.segs.forEach(sg => { if (sg.tile && want.has(sg.i)) { const x = sg.off / MOUSE.total * W; ctx.fillRect(x - 1, 14, 3, 26); ctx.beginPath(); ctx.moveTo(x - 5, 2); ctx.lineTo(x + 5, 2); ctx.lineTo(x, 10); ctx.fill(); } });
    ctx.fillStyle = cols.s;
    MOUSE.segs.forEach(sg => { if (sg.c === "s" || sg.c === "m") { const x = sg.off / MOUSE.total * W; ctx.fillRect(x - 1, 14, 3, 26); } });
  }
  labels.forEach((l, i) => { const b = h("button", { type: "button", "aria-pressed": "false" }, l); b.onclick = () => show(i); $("#zrSteps").append(b); });
  show(0); addEventListener("resize", drawMap); themeListeners.push(drawMap);
})();

/* ---------- figure: how an assistant looks ---------- */
(function look() {
  const shots = [
    { call: `openreadout_preview <b>{"file": "young-mouse.czi"}</b>`, t: "The whole section fits in one small picture, with rulers in full-resolution pixels. The dark rectangles on the right look like areas the scanner skipped, not missing tissue. Dense tissue near the snout sits around x 24,000 to 42,000, y 48,000 to 60,000: a good place to test sharpness." },
    { call: `openreadout_preview <b>{"file": "young-mouse.czi", "region": {"x": 24000, "y": 48000, "width": 18000, "height": 12000}}</b>`, t: "Layers of tissue are clear at this scale, but single cells are not. The dense band around x 33,000, y 56,000 is small enough to view at full resolution." },
    { call: `openreadout_preview <b>{"file": "young-mouse.czi", "region": {"x": 31000, "y": 54000, "width": 5000, "height": 3400}}</b>`, t: "At full resolution, 0.22 µm per pixel, individual nuclei are crisp. The scan is sharp enough to count cells; no rescan needed." },
  ];
  shots.forEach((sh, i) => {
    const el = h("div", { class: "shot" });
    el.append(h("span", { class: "n" }, `${i + 1} of 3`), h("div", { class: "call" }, sh.call), h("img", { src: BASE + LAB.look[i], alt: `Preview ${i + 1} with rulers in full-resolution pixels`, loading: "lazy" }), h("p", { class: "thought" }, sh.t));
    $("#look").append(el);
  });
})();

/* ---------- figure: small multiples ---------- */
(function multi() {
  const DESC = {
    czi: "A header, a description, 8,529 image tiles, and the catalogue at the very end.",
    raw: "A header and the instrument method, then 12,107 scans in one stream, then an index to them.",
    qptiff: "Five pages: the full scan and smaller copies of it, each cut into tiles.",
    lif: "A long description written as UTF-16 text, then one block holding every pixel.",
    nd2: "101 frames compressed as JPEG 2000, followed by 105 small metadata records.",
    fcs: "A fixed-width header, a list of key–value pairs, then 290,172 events.",
    dm3: "One tree of named tags that holds everything, the picture and a thumbnail included.",
    abf: "A header, sections describing the protocol, then 17 sweeps of voltage samples.",
    opus: "A directory, blocks of instrument parameters, and seven blocks of spectra.",
  };
  const fig = $("#f-multi"), tip = $("#multiTip"), rows = [];
  MAPS.multi.forEach(f => {
    const row = h("div", { class: "mrow" });
    row.append(h("div", { class: "who" }, `<b>${f.who}</b><span>.${f.ext} · ${fmtBytes(f.total)}</span>`));
    const cv = h("canvas"); row.append(cv);
    row.append(h("div", { class: "desc" }, DESC[f.ext] || ""));
    $("#multi").append(row);
    const segs = f.segs.map(([off, size, c, kind, name, n]) => ({ off, size, c, kind, name, n })).sort((a, b) => b.size - a.size);
    rows.push({ cv, f, segs });
    cv.addEventListener("pointermove", e => {
      const r = cv.getBoundingClientRect(), b = (e.clientX - r.left) / r.width * f.total;
      const hitSeg = [...segs].reverse().find(s => s.off <= b && b < s.off + s.size);
      if (!hitSeg) { tip.hidden = true; return; }
      const fr = fig.getBoundingClientRect();
      tipAt(tip, fig, e.clientX - fr.left, r.top - fr.top, `<b>${hitSeg.name || hitSeg.kind}</b>${hitSeg.n > 1 ? ` and ${fmtInt(hitSeg.n - 1)} more like it` : ""}<br>${hitSeg.kind} · ${fmtBytes(hitSeg.size)}`);
    });
    cv.addEventListener("pointerleave", () => tip.hidden = true);
  });
  function draw() {
    const cols = COLOR(fig);
    rows.forEach(({ cv, f, segs }) => {
      const W = cv.clientWidth, ctx = hiDPI(cv, W, 26);
      ctx.fillStyle = cols.panel2; ctx.fillRect(0, 0, W, 26);
      segs.forEach(sg => { const x = sg.off / f.total * W, w = Math.max(sg.size / f.total * W, 1); ctx.fillStyle = cols[sg.c]; ctx.globalAlpha = sg.c === "d" && /pyramid/.test(sg.kind) ? .45 : 1; ctx.fillRect(x, sg.c === "m" && sg.size > f.total * .9 ? 0 : 3, w, sg.c === "m" && sg.size > f.total * .9 ? 26 : 20); });
      ctx.globalAlpha = 1;
    });
  }
  draw(); addEventListener("resize", draw); themeListeners.push(draw);
})();

/* ---------- the format list ---------- */
(function lab() {
  const L = LAB, FAMN = ["Light microscopy", "Electron microscopy", "Mass spectrometry", "Chromatography", "Infrared and Raman", "NMR", "Flow cytometry", "Electrophysiology", "qPCR", "Plate readers"];
  const n = L.rows.length, damaged = L.rows.filter(r => r[3] >= 2).length;
  $("#labBig").textContent = `${fmtInt(n)} datasets in ${fmtInt(L.totals.files)} files, ${Math.round(L.totals.dataset_bytes / 1e9)} GB from ten kinds of instrument, recorded between ${L.years[0]} and 2026`;
  $("#labSecs").textContent = `${L.index_seconds} seconds on a laptop`;
  $("#health").textContent = `${damaged} datasets that are truncated, corrupt or unreadable; one exact duplicate and ${L.near} groups of near-duplicates, ${L.exported_twice} of them the same experiment exported twice; ${L.pii} datasets that name a person, which matters before anything is shared; and ${fmtInt(L.at_risk.count)} datasets, ${(L.at_risk.bytes / 1e9).toFixed(1)} GB, that exist only in a maker’s own format with no open copy beside them.`;
  $("#batchN").textContent = L.batch.files;
  const cells = [];
  L.families.forEach((fam, fi) => {
    const rows = L.rows.map((r, i) => [r, i]).filter(([r]) => r[0] === fi).sort((a, b) => b[0][1] - a[0][1]);
    if (!rows.length) return;
    const cols = Math.max(6, Math.min(22, Math.ceil(Math.sqrt(rows.length) * 1.5)));
    const g = h("div", { class: "fam" }), grid = h("div", { class: "cells", style: `--cols:${cols}` });
    rows.forEach(([r, i]) => { const c = h("i", { title: `${r[7]} · ${fmtBytes(r[1])}${r[2] ? " · " + r[2] : ""}` }); cells[i] = c; grid.append(c); });
    g.append(grid, h("span", {}, `<b>${FAMN[fi]}</b> ${rows.length}`));
    $("#tray").append(g);
  });
  const lenses = [
    ...L.queries.map((q, i) => ({ label: q.q, text: q.q, test: r => (r[6] >> i) & 1, cls: "on" })),
    { label: "damaged or unreadable", text: "health → truncated, corrupt or unreadable", test: r => r[3] >= 2, cls: "bad", sep: true },
    { label: "names a person", text: "health → personal data", test: r => r[4], cls: "on" },
    { label: "duplicates", text: "health → duplicates and exports of the same experiment", test: r => r[5], cls: "on" },
  ];
  function apply(k) {
    const ln = lenses[k]; let hits = 0;
    L.rows.forEach((r, i) => { const on = !!ln.test(r); hits += on; cells[i].className = on ? ln.cls : "dim"; });
    $("#qText").textContent = ln.text;
    $("#qOut").textContent = `${fmtInt(hits)} of ${fmtInt(n)} datasets`;
    [...$("#presets").querySelectorAll("button")].forEach((b, j) => b.setAttribute("aria-pressed", String(j === k)));
  }
  lenses.forEach((ln, k) => {
    if (ln.sep) $("#presets").append(h("span", { class: "sep" }));
    const b = h("button", { type: "button", class: "chipbtn", "aria-pressed": "false" }); b.textContent = ln.label; b.onclick = () => apply(k); $("#presets").append(b);
  });
  apply(0);
  const t = L.table, lab2 = ["recording", "sweep", "current (pA)", "spikes", "rate (Hz)", "half-width (ms)"];
  const rows = t.rows.filter(r => r[4] !== null);
  $("#tidy").innerHTML = `<thead><tr>${lab2.map(x => `<th>${x}</th>`).join("")}</tr></thead><tbody>${rows.map(r => `<tr><td>${r[0].replace(/^pyabf-/, "")}</td><td>${r[1]}</td><td>${r[2]}</td><td>${r[3]}</td><td>${r[4]}</td><td>${r[5].toFixed(2)}</td></tr>`).join("")}</tbody>`;
})();

/* ---------- waffles ---------- */
