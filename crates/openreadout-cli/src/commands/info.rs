//! `info`: what a file holds, in five views (summary, full, structure, explain, format), or
//! one metadata row per data set with `--tidy`.

use std::path::{Path, PathBuf};

use clap::ValueEnum;
use openreadout_core::model::{
    DetectOutput, Dump, FileInfo, ImageInfo, Listing, SpectraInfo, TableInfo, TraceInfo,
};
use openreadout_core::{Error, InfoOutput, Registry, Result};
use openreadout_ops::explain::Explanation;

use super::batch::{self, Spec, Stdin};
use super::{human_bytes, live_err, plate, render_assurance_line, sidecar, tidy, truncate};
use crate::output::fail;

/// What `info` shows.
#[derive(Debug, Clone, Copy, Default, ValueEnum, PartialEq, Eq)]
pub enum InfoView {
    /// The normalized summary.
    #[default]
    Summary,
    /// The summary with per-frame records, plus the vendor's metadata tree and per-field provenance.
    Full,
    /// The container's structure: images, planes, segments/chunks/blocks, attachments, pyramid levels.
    Structure,
    /// Plain English for non-experts: what was measured, how, when, what is unusual, what to run next.
    Explain,
    /// The format only, from the file's signature.
    Format,
}

/// Arguments of `info`.
#[derive(Debug, clap::Args)]
pub struct InfoArgs {
    /// Files, directories or glob patterns; several make a batch (see `--recursive`,
    /// `--jsonl`). `-` reads standard input.
    #[arg(required_unless_present = "from_index", value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: batch::BatchArgs,
    #[arg(long)]
    pub json: bool,
    /// What to show.
    #[arg(long, value_enum, default_value_t)]
    pub view: InfoView,
    /// A question in plain words ("what was the gradient?", "which channel is DAPI?", "what
    /// is the sample id?"), answered from the experiment model under `answers`. Implies
    /// `--view explain`.
    #[arg(long, value_name = "QUESTION")]
    pub ask: Option<String>,
    /// List at most N images (0 = all). Default: every image, except screening plates,
    /// whose field images are all alike: the first 16 (`images_total` and a note say how many
    /// there are; `plate.wells[].images` indexes every field).
    #[arg(long, value_name = "N")]
    pub max_images: Option<usize>,
    /// `--view full`: omit the (often large) vendor metadata tree.
    #[arg(long, help_heading = "--view full")]
    pub no_vendor: bool,
    /// `--view full`: omit the provenance map.
    #[arg(long, help_heading = "--view full")]
    pub no_provenance: bool,
    /// `--view full`: embed every per-frame record under `images[].extra.frames` (default:
    /// the first 100 per image).
    #[arg(long, help_heading = "--view full")]
    pub all_frames: bool,
    /// `--view full`: replace values flagged as personal data (operator names, e-mails,
    /// phone numbers, patient-like ids, dates of birth, free-text comments) with stable
    /// salted hashes. Needs a salt: `--salt-file` or OPENREADOUT_REDACT_SALT
    /// (https://openreadout.github.io/openreadout/guides/lab-shares.html#personal-data).
    #[arg(long, conflicts_with = "sidecar", help_heading = "--view full")]
    pub redact: bool,
    /// File holding the redaction salt (never printed or stored).
    #[arg(
        long,
        value_name = "FILE",
        requires = "redact",
        help_heading = "--view full"
    )]
    pub salt_file: Option<PathBuf>,
    /// `--view full`: write each input's full metadata to `<file>.openreadout.json` next to
    /// it (`--sidecar=DIR`: under DIR, mirroring the inputs' relative paths) instead of
    /// printing it; inputs are never modified. An up-to-date sidecar (same source size and
    /// modification time, same options and version) is left as it is.
    #[arg(
        long,
        value_name = "DIR",
        num_args = 0..=1,
        require_equals = true,
        help_heading = "--view full"
    )]
    #[allow(clippy::option_option)] // --sidecar, --sidecar=DIR or neither
    pub sidecar: Option<Option<PathBuf>>,
    #[command(flatten)]
    pub tidy: tidy::TidyArgs,
}

pub fn run(reg: &Registry, a: &InfoArgs) -> i32 {
    let view = if a.ask.is_some() {
        InfoView::Explain
    } else {
        a.view
    };
    let full_only =
        a.no_vendor || a.no_provenance || a.all_frames || a.redact || a.sidecar.is_some();
    if full_only && view != InfoView::Full {
        return fail(
            a.json,
            &Error::Usage(
                "--no-vendor, --no-provenance, --all-frames, --redact and --sidecar go with --view full".into(),
            ),
        );
    }
    if tidy::wanted(reg, &a.tidy, &a.batch, &a.files, false) {
        if view != InfoView::Summary {
            return fail(
                a.json,
                &Error::Usage(format!(
                    "the batch table (--tidy, --fields, --sample-sheet, -o, ...) is the summary view, not --view {}",
                    view.to_possible_value()
                        .map_or_else(String::new, |v| v.get_name().to_string())
                )),
            );
        }
        let m = openreadout_batch::measures::InfoMeasure {
            fields: a.tidy.fields.clone(),
        };
        return tidy::run(reg, &m, &a.files, &a.batch, &a.tidy, a.json);
    }
    let spec = |stdin| Spec {
        json: a.json,
        batch: &a.batch,
        stdin,
    };
    let files = &a.files;
    match view {
        InfoView::Summary => batch::run(
            reg,
            files,
            spec(Stdin::Spool),
            &mut |i| live_err(i.path, || info(reg, i.path, a.max_images)),
            &render_info,
        ),
        InfoView::Format => batch::run(
            reg,
            files,
            spec(Stdin::Pass),
            &mut |i| detect(reg, i.path),
            &|d: &DetectOutput| format!("{}\t{}\t{:?}", d.format, d.name, d.confidence),
        ),
        InfoView::Structure => batch::run(
            reg,
            files,
            spec(Stdin::Spool),
            &mut |i| live_err(i.path, || ls(reg, i.path)),
            &render_ls,
        ),
        InfoView::Explain => batch::run(
            reg,
            files,
            spec(Stdin::Spool),
            &mut |i| explain(reg, i.path, a.ask.as_deref()),
            &render_explanation,
        ),
        InfoView::Full => run_full(reg, a, spec),
    }
}

/// `info --view full`: print the dump (optionally redacted), or write sidecars.
fn run_full<'a>(reg: &Registry, a: &'a InfoArgs, spec: impl Fn(Stdin) -> Spec<'a>) -> i32 {
    let options = sidecar::SidecarOptions {
        vendor: !a.no_vendor,
        provenance: !a.no_provenance,
        all_frames: a.all_frames,
    };
    if let Some(dir) = &a.sidecar {
        return batch::run(
            reg,
            &a.files,
            spec(Stdin::Reject),
            &mut |i| sidecar::write(reg, i, options, dir.as_deref()),
            &sidecar::render,
        );
    }
    let redactor = if a.redact {
        match openreadout_index::pii::load_salt(a.salt_file.as_deref()) {
            Ok(s) => Some(openreadout_index::pii::Redactor::new(s)),
            Err(e) => return fail(a.json, &e),
        }
    } else {
        None
    };
    batch::run(
        reg,
        &a.files,
        spec(Stdin::Spool),
        &mut |i| {
            dump(reg, i.path, options, a.max_images).map(|d| redact_dump(d, redactor.as_ref()))
        },
        &|d: &RedactedDump| serde_json::to_string_pretty(d).expect("serializable"),
    )
}

fn detect(reg: &Registry, file: &Path) -> Result<DetectOutput> {
    let (reader, det) = if file.as_os_str() == "-" {
        let head = batch::stdin_head()?;
        reg.detect_bytes(&head, file)?
    } else {
        reg.detect(file)?
    };
    Ok(DetectOutput {
        path: file.display().to_string(),
        format: det.format_id.into(),
        name: reader.descriptor().name,
        confidence: det.confidence,
        note: det.note,
    })
}

fn info(reg: &Registry, file: &Path, max_images: Option<usize>) -> Result<InfoOutput> {
    let (_, ds) = reg.open(file)?;
    let info = ds.info()?;
    let mut out = InfoOutput::new(ds.as_ref(), info);
    out.acquisition = openreadout_core::live::assess_dataset(ds.as_ref(), file);
    out.cap_images(max_images);
    Ok(out)
}

/// The `info --view full` dump of one file.
pub(crate) fn dump(
    reg: &Registry,
    file: &Path,
    options: sidecar::SidecarOptions,
    max_images: Option<usize>,
) -> Result<Dump> {
    let (_, ds) = reg.open(file)?;
    let mut info = ds.info()?;
    let limit = (!options.all_frames).then_some(openreadout_core::reader::DEFAULT_FRAME_RECORDS);
    openreadout_core::reader::attach_frames(ds.as_ref(), &mut info, limit)?;
    let mut summary = InfoOutput::new(ds.as_ref(), info);
    summary.cap_images(max_images);
    Ok(Dump {
        file: summary,
        vendor: if options.vendor {
            Some(ds.vendor_metadata()?)
        } else {
            None
        },
        provenance: if options.provenance {
            ds.provenance()
        } else {
            Default::default()
        },
    })
}

fn explain(reg: &Registry, file: &Path, ask: Option<&str>) -> Result<Explanation> {
    let (_, ds) = reg.open(file)?;
    let mut info = ds.info()?;
    let experiment = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
    // One record per image is enough to know whether per-frame records exist (and how many).
    let _ = openreadout_core::reader::attach_frames(ds.as_ref(), &mut info, Some(1));
    let assurance = openreadout_core::assurance::assess_dataset(ds.as_ref(), &info);
    Ok(
        openreadout_ops::explain::explain_with(&info, Some(&experiment), ask)
            .with_assurance(assurance),
    )
}

fn render_explanation(e: &Explanation) -> String {
    let mut s = format!("{}\n", e.summary);
    for p in &e.paragraphs {
        s.push('\n');
        s.push_str(p);
        s.push('\n');
    }
    if !e.caveats.is_empty() {
        s.push_str("\nCaveats:\n");
        for c in &e.caveats {
            s.push_str(&format!("  - {c}\n"));
        }
    }
    if !e.answers.is_empty() {
        s.push_str("\nAnswers:\n");
        for a in &e.answers {
            s.push_str(&format!("  [{}] {}\n", a.topic, a.answer));
            if !a.fields.is_empty() {
                s.push_str(&format!("      from: {}\n", a.fields.join(", ")));
            }
        }
    }
    if !e.suggested_commands.is_empty() {
        s.push_str("\nNext steps:\n");
        for c in &e.suggested_commands {
            s.push_str(&format!("  {c}\n"));
        }
    }
    s.trim_end().to_string()
}

fn ls(reg: &Registry, file: &Path) -> Result<Listing> {
    let (det, ds) = reg.open(file)?;
    Ok(Listing {
        path: file.display().to_string(),
        format: det.format_id.into(),
        entries: ds.entries()?,
        acquisition: openreadout_core::live::assess_dataset(ds.as_ref(), file),
    })
}

/// `info --view full` output, with personal data replaced when `--redact` is given.
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum RedactedDump {
    Plain(Box<Dump>),
    Redacted(serde_json::Value),
}

impl batch::Item for RedactedDump {
    fn format(&self) -> Option<String> {
        match self {
            RedactedDump::Plain(d) => d.format(),
            RedactedDump::Redacted(v) => v
                .pointer("/file/format/id")
                .and_then(|f| f.as_str())
                .map(str::to_string),
        }
    }
    fn contents(&self) -> Option<String> {
        match self {
            RedactedDump::Plain(d) => d.contents(),
            RedactedDump::Redacted(_) => Some("redacted".into()),
        }
    }
    fn relabel(&mut self, to: &str) {
        match self {
            RedactedDump::Plain(d) => d.relabel(to),
            RedactedDump::Redacted(v) => v["file"]["path"] = serde_json::Value::from(to),
        }
    }
}

/// Personal data is found in the summary (with its experiment) and in the vendor tree, then
/// every flagged value is replaced everywhere in the dump.
fn redact_dump(d: Dump, r: Option<&openreadout_index::pii::Redactor>) -> RedactedDump {
    let Some(r) = r else {
        return RedactedDump::Plain(Box::new(d));
    };
    let mut v = serde_json::to_value(&d).expect("serializable");
    let mut findings: Vec<_> =
        openreadout_index::pii::scan(&serde_json::to_value(&d.file).unwrap_or_default())
            .into_iter()
            .map(|mut f| {
                f.field = format!("file.{}", f.field);
                f
            })
            .collect();
    if let Some(vendor) = &d.vendor {
        for mut f in openreadout_index::pii::scan(vendor) {
            f.field = format!("vendor.{}", f.field);
            findings.push(f);
        }
    }
    r.redact(&mut v, &findings);
    RedactedDump::Redacted(v)
}

fn render_info(o: &InfoOutput) -> String {
    let i: &FileInfo = o;
    let mut s = if !i.spectra.is_empty() {
        format!(
            "{}\n  format: {} ({}) v{}  size: {}  spectra runs: {}  traces: {}\n",
            i.path,
            i.format.name,
            i.format.id,
            i.format_version.as_deref().unwrap_or("?"),
            human_bytes(i.size_bytes),
            i.spectra.len(),
            i.traces.len()
        )
    } else if !i.traces.is_empty() {
        format!(
            "{}\n  format: {} ({}) v{}  size: {}  traces: {}\n",
            i.path,
            i.format.name,
            i.format.id,
            i.format_version.as_deref().unwrap_or("?"),
            human_bytes(i.size_bytes),
            i.traces.len()
        )
    } else if i.tables.is_empty() {
        format!(
            "{}\n  format: {} ({}) v{}  size: {}  images: {}  planes: {}\n",
            i.path,
            i.format.name,
            i.format.id,
            i.format_version.as_deref().unwrap_or("?"),
            human_bytes(i.size_bytes),
            i.images.len(),
            i.plane_count
        )
    } else {
        format!(
            "{}\n  format: {} ({}) v{}  size: {}  tables: {}\n",
            i.path,
            i.format.name,
            i.format.id,
            i.format_version.as_deref().unwrap_or("?"),
            human_bytes(i.size_bytes),
            i.tables.len()
        )
    };
    if let Some(a) = &o.assurance {
        s.push_str(&render_assurance_line(a));
    }
    // A multi-well plate has one image per field of view: summarize the plate and show only
    // the first field in full (every field is in `--json`).
    let shown = if let Some(p) = &o.plate {
        s.push_str(&plate::render_plate_summary(p, i.images.len()));
        1
    } else {
        usize::MAX
    };
    for im in i.images.iter().take(shown) {
        render_image(&mut s, im);
    }
    for r in &i.spectra {
        render_spectra_run(&mut s, r);
    }
    for t in &i.tables {
        render_table(&mut s, t);
    }
    for t in &i.traces {
        render_trace_info(&mut s, t);
    }
    for n in &i.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s.trim_end().to_string()
}

/// One image of `info`: geometry, channels, objective, acquisition time.
fn render_image(s: &mut String, im: &ImageInfo) {
    s.push_str(&format!(
        "  [{}] {}  {}x{} z={} c={} t={}  {} {}  px={}\n",
        im.index,
        im.name.as_deref().unwrap_or("-"),
        im.size_x,
        im.size_y,
        im.size_z,
        im.size_c,
        im.size_t,
        im.pixel_type.ome_name(),
        im.dimension_order,
        im.physical_size.x.map_or_else(
            || "?".into(),
            |x| format!("{x:.4} {}", im.physical_size.unit)
        )
    ));
    // hyperspectral maps have a channel per spectral point: list the first ones only
    if im.channels.len() > 32 {
        s.push_str(&format!(
            "      {} channels: {} … {} (`--json` lists them)\n",
            im.channels.len(),
            im.channels[0].name.as_deref().unwrap_or("-"),
            im.channels[im.channels.len() - 1]
                .name
                .as_deref()
                .unwrap_or("-")
        ));
    }
    for c in im
        .channels
        .iter()
        .take(if im.channels.len() > 32 { 0 } else { 32 })
    {
        s.push_str(&format!(
            "      ch{} {}{}\n",
            c.index,
            c.name.as_deref().unwrap_or("-"),
            c.emission_range_nm
                .map(|[a, b]| format!("  {a:.0}-{b:.0} nm"))
                .unwrap_or_default()
        ));
    }
    if let Some(o) = &im.objective {
        s.push_str(&format!(
            "      objective: {} {}x NA {}\n",
            o.model.as_deref().unwrap_or("-"),
            o.nominal_magnification.unwrap_or(0.0),
            o.lens_na.unwrap_or(0.0)
        ));
    }
    if let Some(t) = &im.acquired_at {
        s.push_str(&format!("      acquired: {t}\n"));
    }
}

/// One mass-spectrometry run of `info`.
fn render_spectra_run(s: &mut String, r: &SpectraInfo) {
    s.push_str(&format!(
        "  run [{}] {}  {} spectra  MS levels {:?}{}\n",
        r.index,
        r.name.as_deref().unwrap_or("-"),
        r.scan_count,
        r.ms_levels,
        r.rt_range_s
            .map(|[a, b]| format!("  rt {a:.2}-{b:.2} s"))
            .unwrap_or_default()
    ));
    if let Some(ins) = &r.instrument {
        let parts: Vec<&str> = [&ins.manufacturer, &ins.model, &ins.software]
            .into_iter()
            .filter_map(|v| v.as_deref())
            .collect();
        if !parts.is_empty() {
            s.push_str(&format!("      instrument: {}\n", parts.join(" / ")));
        }
    }
    if let Some(a) = r.extra.get("acquired_at").and_then(|v| v.as_str()) {
        s.push_str(&format!("      acquired: {a}\n"));
    }
}

/// One table of `info`: rows, columns, instrument and spillover.
fn render_table(s: &mut String, t: &TableInfo) {
    let str_extra = |k: &str| t.extra.get(k).and_then(|v| v.as_str()).map(str::to_string);
    s.push_str(&format!(
        "  table [{}] {}  {} rows x {} columns{}\n",
        t.index,
        t.name.as_deref().unwrap_or("-"),
        t.row_count,
        t.columns.len(),
        t.extra
            .get("datatype")
            .and_then(|v| v.as_str())
            .map(|d| format!("  $DATATYPE {d}"))
            .unwrap_or_default()
    ));
    if let Some(m) = t
        .extra
        .get("instrument")
        .and_then(|v| v.get("model"))
        .and_then(|v| v.as_str())
    {
        s.push_str(&format!("      instrument: {m}\n"));
    }
    if let Some(sw) = str_extra("software") {
        s.push_str(&format!("      software: {sw}\n"));
    }
    if let Some(a) = str_extra("acquisition_start").or_else(|| str_extra("acquisition_date")) {
        s.push_str(&format!("      acquired: {a}\n"));
    }
    for c in &t.columns {
        s.push_str(&format!(
            "      {:>3} {:<16} {:<28} {}\n",
            c.index,
            truncate(&c.name, 16),
            truncate(c.label.as_deref().unwrap_or("-"), 28),
            c.dtype
        ));
    }
    if let Some(sp) = t.extra.get("spillover") {
        let n = sp
            .get("parameters")
            .and_then(|p| p.as_array())
            .map_or(0, Vec::len);
        s.push_str(&format!(
            "      spillover: {n}x{n} matrix from {}\n",
            sp.get("keyword").and_then(|k| k.as_str()).unwrap_or("?")
        ));
    }
}

/// One trace of `info`: sweeps, axis, NMR facts, channels and tags.
fn render_trace_info(s: &mut String, t: &TraceInfo) {
    let str_extra = |k: &str| t.extra.get(k).and_then(|v| v.as_str()).map(str::to_string);
    s.push_str(&format!(
        "  trace [{}] {}  {} sweep(s) x {} samples  {} channel(s)  {} Hz\n",
        t.index,
        t.name.as_deref().unwrap_or("-"),
        t.sweep_count,
        t.sample_count,
        t.channels.len(),
        t.sample_rate_hz
    ));
    if let Some(m) = str_extra("acquisition_mode") {
        s.push_str(&format!("      mode: {m}\n"));
    }
    if let Some(a) = str_extra("created_at").or_else(|| str_extra("recorded_at")) {
        s.push_str(&format!("      recorded: {a}\n"));
    }
    if let Some(a) = t
        .extra
        .get("axis")
        .filter(|a| a.get("quantity").and_then(|q| q.as_str()) != Some("time"))
    {
        let num = |k: &str| a.get(k).and_then(serde_json::Value::as_f64);
        if let (Some(first), Some(step)) = (num("first"), num("step")) {
            let last = first + step * (t.sample_count.saturating_sub(1)) as f64;
            s.push_str(&format!(
                "      axis: {} {first:.6} .. {last:.6} {}\n",
                a.get("quantity").and_then(|v| v.as_str()).unwrap_or("x"),
                a.get("unit").and_then(|v| v.as_str()).unwrap_or("")
            ));
        }
    }
    let mut facts = Vec::new();
    for (k, label) in [
        ("nucleus", "nucleus"),
        ("pulse_program", "pulse program"),
        ("solvent", "solvent"),
        ("data_type", "data type"),
        ("instrument", "instrument"),
        ("acquired_at", "acquired"),
    ] {
        if let Some(v) = str_extra(k) {
            facts.push(format!("{label}: {v}"));
        }
    }
    if let Some(f) = t
        .extra
        .get("spectrometer_frequency_mhz")
        .and_then(serde_json::Value::as_f64)
    {
        facts.push(format!("{f:.4} MHz"));
    }
    if !facts.is_empty() {
        s.push_str(&format!("      {}\n", facts.join("  ")));
    }
    for c in t.channels.iter().take(64) {
        s.push_str(&format!(
            "      {:>3} {:<24} {:<8} {}\n",
            c.index,
            truncate(&c.name, 24),
            c.unit.as_deref().unwrap_or("-"),
            c.dtype
        ));
    }
    if t.channels.len() > 64 {
        s.push_str(&format!(
            "      … {} more channels (see --json)\n",
            t.channels.len() - 64
        ));
    }
    if let Some(tags) = t.extra.get("tags").and_then(|v| v.as_array()) {
        for tag in tags.iter().take(10) {
            s.push_str(&format!(
                "      tag @{:.3} s: {}\n",
                tag.get("time_s")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
                tag.get("comment").and_then(|v| v.as_str()).unwrap_or("")
            ));
        }
    }
}

fn render_ls(l: &Listing) -> String {
    use crate::ui::{self, Align};
    let rows: Vec<Vec<String>> = l
        .entries
        .iter()
        .map(|e| {
            vec![
                e.kind.clone(),
                truncate(&e.name, 48),
                e.offset.map(|o| o.to_string()).unwrap_or_default(),
                e.size.map(human_bytes).unwrap_or_default(),
                e.image.map(|i| i.to_string()).unwrap_or_default(),
            ]
        })
        .collect();
    format!(
        "{} ({}), {} entries\n{}",
        ui::paint(ui::BOLD, &l.path),
        l.format,
        l.entries.len(),
        ui::table(
            &["kind", "name", "offset", "size", "image"],
            &[
                Align::Left,
                Align::Left,
                Align::Right,
                Align::Right,
                Align::Right
            ],
            &rows
        )
    )
}
