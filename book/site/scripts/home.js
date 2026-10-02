// Figures of the home page. Data: site/data/home.json (taken from real public files; see the
// notes at the end of the page).
import D from "../data/home.json";
import { $, h, fmtInt, coverBar, figs } from "./charts.js";
const BASE = import.meta.env.BASE_URL.replace(/\/?$/, "/");

// Gallery labels: format · what the file holds, then a few facts read from it.
const META = {
  stem: { title: "Leica LIF · confocal lambda scan", usual: "Leica LAS X",
    facts: c => [`${c.facts.channels} spectral channels`, "512 × 512 px", "2.27 µm per pixel"] },
  atoms: { title: "Gatan DM3 · atomic-resolution STEM", usual: "Gatan DigitalMicrograph",
    facts: () => ["1024 × 1024 px", "0.0244 nm per pixel"] },
  tissue: { title: "Akoya QPTIFF · H&E whole slide", usual: "Akoya Phenochart",
    facts: () => ["30,720 × 26,640 px", "0.5 µm per pixel", "402 MB"] },
  neuron: { title: "Axon ABF · patch clamp", usual: "Molecular Devices pCLAMP",
    facts: c => [`${c.facts.sweeps} sweeps`, `${fmtInt(c.facts.spikes_total)} spikes`, `${c.facts.rate_khz} kHz`] },
  cannabis: { title: "Thermo Orbitrap RAW · LC-MS run", usual: "Thermo Xcalibur",
    facts: c => [c.facts.model, `${fmtInt(c.facts.scans)} scans`, "24 min"] },
  juice: { title: "Bruker OPUS · FT-IR spectrum", usual: "Bruker OPUS",
    facts: c => [`${fmtInt(c.facts.points)} points`, "4000 → 500 cm⁻¹"] },
  pcr: { title: "Applied Biosystems EDS · qPCR", usual: "Design & Analysis",
    facts: c => [c.facts.instrument, `${c.facts.wells} wells`, `${c.facts.cycles} cycles`] },
  plate: { title: "Tecan Magellan · 384-well ELISA", usual: "Tecan Magellan",
    facts: c => [`${c.facts.wells} wells`, `${c.facts.wavelength} nm`, `${c.facts.reads} reads`] },
  cells: { title: "FCS · flow cytometry", usual: "BD FACSDiva, FlowJo",
    facts: c => [`${fmtInt(c.facts.events)} cells`, `${c.facts.params} parameters`, "6,000 plotted"] },
};

/* ---------- gallery ---------- */
(function gallery() {
  const WIDTH_UM = { stem: [512 * 2.2749510763209395, "on-dark"], atoms: [300 * 2.44140625e-5, "on-dark"], tissue: [30720 * 0.5, "on-light"] };
  D.cards.forEach(cd => {
    if (cd.img) cd.img = BASE + cd.img;
    const mt = META[cd.id]; cd.title = mt.title;
    const el = h("div", { class: "spec" }), fig = h("div", { class: "fig" });
    figs[cd.kind](fig, cd);
    if (WIDTH_UM[cd.id]) coverBar(fig, fig.querySelector("img"), WIDTH_UM[cd.id][0], WIDTH_UM[cd.id][1]);
    el.append(fig, h("h3", {}, mt.title), h("p", {}, mt.facts(cd).join(" · ")), h("div", { class: "who" }, `Usually opened in ${mt.usual}`));
    $("#plate").append(el);
  });
})();

/* ---------- questions an agent can answer ---------- */
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
      ai: ["It's all in the file. Here's a paragraph you can adapt:",
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
