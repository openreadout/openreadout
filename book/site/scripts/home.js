// Figures of the home page. Data: site/data/home.json (taken from real public files; see the
// notes at the end of the page).
import D from "../data/home.json";
import { $, h, s, fmtInt, reduced, css, hexRows, scaleBar, coverBar, figs } from "./charts.js";
const BASE = import.meta.env.BASE_URL.replace(/\/?$/, "/");

const META = {
  stem: { title: "A plant stem, through a laser microscope", machine: "Leica confocal microscope · .lif", usual: "Leica LAS X",
    body: "A slice across the stem of a lily-of-the-valley. The microscope split the light into 29 colour bands; the rings are bundles of tubes that carry water and sugar.",
    facts: c => [`${c.facts.channels} colour bands`, "512 × 512 px", "2.27 µm per pixel"], ask: "Which of the 29 colour bands is brightest?" },
  atoms: { title: "Individual atoms in a crystal", machine: "Gatan electron-microscope camera · .dm3", usual: "Gatan DigitalMicrograph",
    body: "Each bright dot is a column of atoms. The window shown is about 7 nanometres wide, roughly ten thousand times narrower than a human hair.",
    facts: c => ["1024 × 1024 px", "0.0244 nm per pixel", "window: 7.3 nm"], ask: "How wide is this image in nanometres?" },
  tissue: { title: "A tissue slide, as a pathologist sees it", machine: "Akoya / PerkinElmer slide scanner · .qptiff", usual: "Akoya Phenochart",
    body: "Stained pink and purple with H&E, the standard stain in hospital pathology labs. The full scan is 818 million pixels; this preview read only a small, low-resolution copy stored inside the file.",
    facts: c => ["30,720 × 26,640 px", "0.5 µm per pixel", "402 MB"], ask: "Show me the top-left corner at full resolution." },
  neuron: { title: "A brain cell firing", machine: "Axon patch-clamp amplifier · .abf", usual: "Molecular Devices pCLAMP",
    body: "A glass needle touches one neuron and records its voltage 20,000 times a second. Push in a small current and it fires: each spike is one nerve impulse.",
    facts: c => [`${fmtInt(c.facts.spikes_total)} spikes found`, `${c.facts.sweeps} recordings`, `up to ${c.facts.rate_hz} spikes/s`], ask: "How fast does this cell fire when it's stimulated?" },
  cannabis: { title: "Weighing the molecules in a cannabis extract", machine: "Thermo Fisher Orbitrap mass spectrometer · .raw", usual: "Thermo Xcalibur (Windows)",
    body: "The instrument separates the mixture over 24 minutes and weighs whatever comes out, thousands of times. Each peak is a group of molecules leaving at the same moment.",
    facts: c => [`${fmtInt(c.facts.scans)} scans`, c.facts.model, "24 min run"], ask: "When does the THCA peak come out?" },
  juice: { title: "The infrared fingerprint of fruit juice", machine: "Bruker FT-IR spectrometer · OPUS .0", usual: "Bruker OPUS",
    body: "Molecules absorb infrared light at frequencies set by their chemical bonds, so the dips identify what's in the sample. Food labs use this kind of scan to check juice for dilution or added sugar.",
    facts: c => [`${fmtInt(c.facts.points)} points`, "4000 → 500 cm⁻¹", "orange-peach juice"], ask: "Where are the strongest absorption bands?" },
  pcr: { title: "The kind of machine that ran COVID tests", machine: "Applied Biosystems QuantStudio 7 Pro · .eds", usual: "Design & Analysis software",
    body: "It copies DNA over and over and measures the glow. A sample with more DNA lights up sooner, so each 10-fold dilution here rises 3.3 cycles later, as the chemistry predicts.",
    facts: c => [`${c.facts.amplified} wells amplified`, `${c.facts.cycles} cycles`, `rise at cycle ${c.facts.cq_min}–${c.facts.cq_max}`], ask: "Did any wells fail to amplify?" },
  plate: { title: "384 tiny experiments on one plate", machine: "Tecan plate reader · Magellan export", usual: "Tecan Magellan",
    body: "A plate reader measures the colour of 384 wells at once. This one is an ELISA, the lab test behind many antibody tests: the darker a well, the more of the target it holds.",
    facts: c => [`${c.facts.wells} wells`, `${c.facts.wavelength} nm`, `${c.facts.reads} reads`], ask: "Which wells are above the cut-off?" },
  cells: { title: "290,172 immune cells, measured one at a time", machine: "BD LSR II flow cytometer · .fcs", usual: "BD FACSDiva, FlowJo",
    body: "Cells stream single-file past lasers and each is measured for 15 things. Plotting two markers splits the T cells into the two kinds your immune system relies on.",
    facts: c => [`${fmtInt(c.facts.events)} cells`, `${c.facts.params} measurements each`, "6,000 shown"], ask: "What share of these T cells are helper cells?" },
};
(function decode() {
  const m = D.mouse, UM = 0.22, WMM = m.size_x * UM / 1000, HMM = m.size_y * UM / 1000;
  $("#gpx").textContent = `${fmtInt(m.size_x)} by ${fmtInt(m.size_y)} pixels, ${(m.size_x * m.size_y / 1e9).toFixed(1)} billion`;
  const cv = $("#hero"), ctx = cv.getContext("2d"), wrap = $("#cwrap"), range = $("#decodeRange");
  const img = new Image(), hexStr = m.hex.toUpperCase();
  let off, px, W, H, cw, ch, cols, rows, noise = [], p = 1, anim = null;
  function setup() {
    const r = cv.getBoundingClientRect(), dpr = Math.min(window.devicePixelRatio || 1, 2);
    W = Math.max(280, Math.round(r.width)); H = Math.round(W * 542 / 1487);
    cv.width = W * dpr; cv.height = H * dpr; ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    off = document.createElement("canvas"); off.width = W; off.height = H;
    const o = off.getContext("2d"); o.drawImage(img, 0, 0, W, H); px = o.getImageData(0, 0, W, H).data;
    const fs = Math.max(6, Math.min(11, W / 120));
    ctx.font = `${fs}px "IBM Plex Mono", ui-monospace, Menlo, monospace`;
    cw = ctx.measureText("0").width + 0.5; ch = fs * 1.35; cols = Math.ceil(W / cw); rows = Math.ceil(H / ch);
    noise = Array.from({ length: rows }, (_, i) => (Math.sin(i * 12.9898) * 43758.5453) % 1);
    scaleBar($("#heroBar"), WMM * 1000 / W, W * 0.14); draw();
  }
  function draw() {
    const band = W * 0.16, bg = css("--slide", cv), ink = css("--hex", cv);
    ctx.fillStyle = bg; ctx.fillRect(0, 0, W, H);
    for (let r = 0; r < rows; r++) {
      const y = r * ch, edge = p * (W + band * 2) - band + Math.abs(noise[r]) * band * 0.7 - band * 0.35;
      if (edge > 0) ctx.drawImage(off, 0, y, Math.min(edge, W), ch, 0, y, Math.min(edge, W), ch);
      for (let c = Math.max(0, Math.floor(edge / cw)); c < cols; c++) {
        const x = c * cw, d = x - edge; let color = ink;
        if (d < band) { const i = ((Math.min(H - 1, Math.floor(y + ch / 2)) * W) + Math.min(W - 1, Math.floor(x))) * 4; color = d / band < 0.6 ? `rgb(${px[i]},${px[i + 1]},${px[i + 2]})` : ink; }
        ctx.fillStyle = color; ctx.fillText(hexStr[(r * cols + c) % hexStr.length], x, y + ch * 0.8);
      }
    }
    $("#heroBar").style.opacity = p > .98 ? 1 : 0;
  }
  range.addEventListener("input", () => { cancelAnimationFrame(anim); p = range.value / 1000; draw(); });
  wrap.addEventListener("pointermove", e => {
    const r = wrap.getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top, ro = $("#ro");
    ro.hidden = false;
    ro.textContent = p > .5 ? `${(x / r.width * WMM).toFixed(2)} mm, ${(y / r.height * HMM).toFixed(2)} mm` : `byte ${fmtInt(Math.floor((y / r.height * rows | 0) * cols + x / r.width * cols))}`;
    ro.style.left = Math.min(x, r.width - 150) + "px"; ro.style.top = Math.min(y, r.height - 30) + "px";
  });
  wrap.addEventListener("pointerleave", () => $("#ro").hidden = true);
  img.onload = () => {
    setup();
    if (reduced) return;
    p = 0; range.value = 0; draw();
    setTimeout(() => {
      const t0 = performance.now();
      const step = now => { const k = Math.min(1, (now - t0) / 2200), e = k < .5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2; p = e; range.value = Math.round(p * 1000); draw(); if (k < 1) anim = requestAnimationFrame(step); };
      anim = requestAnimationFrame(step);
    }, 700);
  };
  img.src = BASE + m.overview;
  let rt; addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(() => img.complete && setup(), 150); });
})();

/* ---------- figure: the file map ---------- */
(function plate() {
  const WIDTH_UM = { stem: [512 * 2.2749510763209395, "on-dark"], atoms: [300 * 2.44140625e-5, "on-dark"], tissue: [30720 * 0.5, "on-light"] };
  const hexes = [];
  D.cards.forEach(cd => {
    if (cd.img) cd.img = BASE + cd.img;
    const mt = META[cd.id]; cd.title = mt.title;
    const el = h("div", { class: "spec" }), fig = h("div", { class: "fig" });
    figs[cd.kind](fig, cd);
    if (WIDTH_UM[cd.id]) coverBar(fig, fig.querySelector("img"), WIDTH_UM[cd.id][0], WIDTH_UM[cd.id][1]);
    const hv = h("pre", { class: "hexview", hidden: "", "aria-label": "First bytes of the file" }, hexRows(cd.hex, 13));
    fig.append(hv); hexes.push(hv);
    el.append(fig, h("h3", {}, mt.title), h("p", {}, mt.body), h("div", { class: "who" }, `${mt.machine} · normally opened in ${mt.usual}`));
    $("#plate").append(el);
  });
  const set = on => { hexes.forEach(x => x.hidden = !on); $("#pBytes").setAttribute("aria-pressed", String(on)); $("#pDecoded").setAttribute("aria-pressed", String(!on)); };
  $("#pBytes").onclick = () => set(true); $("#pDecoded").onclick = () => set(false);
})();

/* ---------- a whole lab ---------- */
(function convos() {
  const esc = x => String(x).replace(/&/g, "&amp;").replace(/</g, "&lt;");
  const n0 = v => fmtInt(Math.round(v));
  const C = D.convos;
  const call = (name, args, exit) => ({ cls: "call", html: `${name} <span class="dim">${esc(JSON.stringify(args))}</span>${exit ? `<span class="exit">→ ${exit.text}</span>` : ""}` });
  const line = (text, cls = "") => ({ cls, html: esc(text) });

  // 1. damaged folder
  const clean = x => x.replace(/`?\/[^`\s]*plate7_run3\.czi`?/g, "plate7_run3.czi");
  const chk = [call("openreadout_check", { file: "plate7/" }, { text: "exit 4 · corrupt" })];
  let shortBy = null;
  D.folder_check.forEach(f => {
    if (f.ok) {
      const n = f.findings.find(x => x.code === "subblocks_checked");
      chk.push({ cls: "", html: `<span class="ok">ok  </span> ${esc(f.file)}  <span class="dim">${n ? esc(n.message) : ""}</span>` });
    } else {
      const errs = f.findings.filter(x => x.severity === "error");
      chk.push({ cls: "err", html: `FAIL ${esc(f.file)}  <span class="dim">${errs.length} problems</span>` });
      errs.slice(0, 4).forEach(x => chk.push(line("     " + clean(x.message), "err")));
      const m = errs.map(x => x.message.match(/by (\d+) bytes/)).find(Boolean);
      if (m) shortBy = +m[1];
    }
  });
  const ex = C.explain, qc = C.qpcr, ep = C.ephys, cmp = C.compare, xp = C.export;
  const acid = C.acid, neu = C.neutral;
  const list = [
    { tab: "Damaged files",
      user: "Before I send the <code>plate7</code> folder to our collaborators, is anything broken?",
      tools: [chk],
      ai: [`One file is damaged. <b>plate7_run3.czi</b> stops partway through: a block of image data runs <b>${shortBy ? fmtInt(shortBy) + " bytes" : "past"}</b> past the end of the file, so the copy was probably interrupted. Image 1 is missing one of its 12 image planes, and the file's table of contents is gone.`,
           "The other two files passed every check. I'd copy <b>run3</b> again from the microscope PC before sending." ],
      note: "We cut one of three microscope files off at 60% to mimic a copy that stopped halfway." },
    { tab: "Settings for a paper",
      user: "What settings was <code>run42.czi</code> recorded with? I need them for the methods section of my paper.",
      tools: [[call("openreadout_explain", { file: "run42.czi" }), line(ex.summary), line(ex.paragraphs[0].replace(/; operator \S+/, "")), line(ex.paragraphs[2]), line(ex.paragraphs[3])]],
      ai: ["Everything is recorded in the file. Here's a paragraph you can adapt:",
           "<blockquote>Images were acquired on a Zeiss Axio Observer.Z1 spinning-disk confocal microscope with a Plan-Apochromat 20×/0.8 air objective, using ZEN 2.3 (blue edition). EGFP and TagRFP were excited at 488 nm and 558 nm with 10 ms exposures, alongside a brightfield channel. At each of three stage positions, z-stacks of 5 planes 1 µm apart were collected at 1.08 µm per pixel.</blockquote>" ] },
    { tab: "Convert for Fiji",
      user: "Can you convert <code>stem.lif</code> so my collaborator can open it in Fiji? Make sure nothing gets lost.",
      tools: [
        [call("openreadout_export", { file: "stem.lif", output: "stem.ome.tiff" }, { text: "verified", good: true }),
         line(`wrote ${xp.output} · ${xp.images_written} image · ${xp.planes_written} planes · ${(xp.bytes_written / 1e6).toFixed(1)} MB · ${xp.codec} (lossless) · read back and verified`)],
        [call("openreadout_compare", { a: "stem.lif", b: "stem.ome.tiff" }, { text: "identical", good: true }),
         line(`geometry equal · channel names equal · pixel size equal`, "ok"),
         line(`pixels: ${cmp.planes.identical} of ${cmp.planes.planes} planes identical, ${cmp.planes.mismatched} mismatched`, "ok"),
         line(`metadata: ${cmp.difference_count} differences (29 colour bands, objective, immersion, software, acquisition mode all kept)`, "ok")] ],
      ai: [`Done: <b>stem.ome.tiff</b> is ${(xp.bytes_written / 1e6).toFixed(1)} MB, about half the original, compressed without any loss. All ${cmp.planes.planes} image planes are pixel-for-pixel identical to the original, and the image size, channel names and pixel scale match.`,
           "The settings came along too: the wavelength band of each of the 29 colour channels (420–440 nm, 430–450 nm, and so on), the objective, the acquisition mode and the software version are all in the OME-TIFF, and a comparison with the original finds no differences. Fiji will open it directly." ] },
    { tab: "PCR quality check",
      user: "Did the standard curve on this QuantStudio run pass?",
      tools: [[call("openreadout_qpcr", { file: "tb18s_run1.eds", standard_curve: true }),
        line(`target ${qc.target} · ${qc.levels} dilution levels · ${qc.points} wells`),
        line(`slope ${qc.slope.toFixed(3)} · R² ${qc.r2.toFixed(4)} · efficiency ${qc.efficiency_percent.toFixed(1)} %`, "ok"),
        line(`vendor's own result: slope ${qc.vendor_slope} · R² ${qc.vendor_r2.toFixed(4)} · efficiency ${qc.vendor_efficiency_percent.toFixed(1)} %`, "dim")]],
      ai: [`Yes. The efficiency is <b>${qc.efficiency_percent.toFixed(1)}%</b>, inside the 90–110% range most labs accept, and R² is ${qc.r2.toFixed(4)}, so the ${qc.levels} dilutions fall almost perfectly on a line.`,
           `A slope of ${qc.slope.toFixed(2)} means each 10-fold dilution shows up ${(-qc.slope).toFixed(2)} cycles later, close to the ideal 3.32. OpenReadout recomputed the curve from the well results, and it matches what the QuantStudio software reported.` ] },
    { tab: "Cannabis extract",
      user: "Is there THCA or CBDA in <code>extract.raw</code>? And was the plant material heated?",
      tools: [
        [call("openreadout_peaks", { file: "extract.raw", mz: [357.2071], ppm: 5, ms_level: 1 }),
         line(`XIC 357.2071 ±5 ppm · ${C.acid_n} peaks, largest:`),
         ...acid.slice(0, 2).map(p => line(`  ${p.rt_min.toFixed(2)} min   area ${p.area_percent.toFixed(1)} %   S/N ${n0(p.snr)}`, "ok"))],
        [call("openreadout_peaks", { file: "extract.raw", mz: [313.2173], ppm: 5, ms_level: 1 }),
         line(`XIC 313.2173 ±5 ppm · ${C.neutral_n} peaks, largest:`),
         line(`  ${neu[0].rt_min.toFixed(2)} min   area ${n0(neu[0].area)}   (largest acid peak: ${n0(acid[0].area)})`, "dim")] ],
      ai: [`Yes, a lot of it. THCA and CBDA share a chemical formula, so they weigh exactly the same, and at that weight there are two large peaks: <b>${acid[0].rt_min.toFixed(2)} min</b> (${acid[0].area_percent.toFixed(0)}% of the signal) and <b>${acid[1].rt_min.toFixed(2)} min</b> (${acid[1].area_percent.toFixed(0)}%). Weight alone can't say which is which; running THCA and CBDA reference standards on the same method would settle it.`,
           `The heated forms, THC and CBD, are about ${fmtInt(Math.round(acid[0].area / neu[0].area / 100) * 100)} times weaker. Heat turns the acids into THC and CBD, so this material looks unheated.` ] },
    { tab: "What kind of neuron?",
      user: "What kind of neuron did I record in <code>cell_0055.abf</code>?",
      tools: [[call("openreadout_ephys_features", { file: "cell_0055.abf" }),
        line(`current clamp · 17 sweeps · ${fmtInt(ep.spikes)} action potentials`),
        line(`resting ${ep.cell.resting_mv.toFixed(1)} mV · rheobase ${ep.cell.rheobase_pa} pA · input resistance ${ep.cell.input_resistance_mohm.toFixed(0)} MΩ`),
        line(`sweep ${ep.sweep.sweep} (${ep.sweep.stimulus_pa} pA): ${ep.sweep.firing_rate_hz} Hz · half-width ${ep.sweep.half_width_mean_ms.toFixed(2)} ms · adaptation index ${ep.sweep.adaptation_index.toFixed(3)}`, "ok")]],
      ai: [`Its electrical behaviour matches a <b>fast-spiking interneuron</b>. The spikes are very narrow (${ep.sweep.half_width_mean_ms.toFixed(2)} ms at half height), and under a steady push it fires up to ${ep.sweep.firing_rate_hz} times a second with almost no slowing down (adaptation index ${ep.sweep.adaptation_index.toFixed(3)}).`,
           "Typical excitatory neurons have wider spikes and slow down during a long current step. Confirming the type would need the cell's shape or a marker, but the electrical fingerprint points clearly that way." ] },
  ];


  const QUESTIONS = ["Is anything in this folder broken?", "What settings was this recorded with?", "Convert it for Fiji without losing anything", "Did the qPCR standard curve pass?", "Is there THCA or CBDA in this extract?", "What kind of neuron did I record?"];
  const qs = $("#qs"), play = $("#play");
  function show(i) {
    const c = list[i];
    [...qs.children].forEach((b, k) => b.setAttribute("aria-selected", String(k === i)));
    play.innerHTML = "";
    const line = (who, node) => { const l = h("div", { class: "line" }); l.append(h("span", { class: "who" }, who)); const say = h("div", { class: "say" }); say.append(node); l.append(say); play.append(l); };
    line("You", h("p", {}, c.user));
    const tw = h("div");
    c.tools.forEach(block => { const tb = h("div", { class: "tool" }); block.forEach(l => tb.append(h("span", l.cls ? { class: l.cls } : {}, l.html))); tw.append(tb); });
    line("OpenReadout", tw);
    const ai = h("div"); ai.innerHTML = c.ai.map(p => p.startsWith("<blockquote") ? p : `<p>${p}</p>`).join("");
    line("Assistant", ai);
    if (c.note) play.append(h("p", { class: "note" }, c.note));
  }
  list.forEach((c, i) => { const b = h("button", { type: "button", role: "tab", "aria-selected": "false" }); b.textContent = QUESTIONS[i]; b.onclick = () => show(i); qs.append(b); });
  show(0);
})();
$("#ver").textContent = D.version + ".";
