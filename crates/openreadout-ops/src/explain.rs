//! Plain-English narrative of a file for non-experts (`openreadout info --view explain`).
//!
//! [`explain`] is a pure function of the normalized [`FileInfo`]: it never reads the file,
//! so it says only what `info` knows, in words, and points at the command that shows more.
//! [`explain_with`] also takes the dataset's [`Experiment`] (sample, method, acquisition) and,
//! optionally, a question in plain words (`--ask "what was the gradient?"`): the matching
//! [`Answer`]s are returned under `answers`, each naming the fields it came from.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use openreadout_core::experiment::{Experiment, Quantity};
use openreadout_core::model::{
    ChannelInfo, FileInfo, ImageInfo, SpectraInfo, TableInfo, TraceInfo,
};
use openreadout_core::pixel::PixelType;
use openreadout_core::provenance::Confidence;

/// Output of `info --view explain`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Explanation {
    /// One or two sentences: what the file is and what it holds.
    pub summary: String,
    /// The narrative, one topic per paragraph (contents, channels, optics, timing, what is unusual).
    pub paragraphs: Vec<String>,
    /// Shell commands tailored to this file, most useful first.
    pub suggested_commands: Vec<String>,
    /// Things the reader should not take for granted (reader notes, missing calibration, ...).
    pub caveats: Vec<String>,
    /// Answers to the question asked with `--ask`, one per topic it touches; empty without one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<Answer>,
    /// Whether the file lies inside what its reader has been validated on (as `info` →
    /// `assurance`); its summary is also the first caveat when the file is not validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<openreadout_core::assurance::Assurance>,
}

impl Explanation {
    /// Attach the file's assurance: its summary becomes the first caveat unless the file is
    /// validated, and a strict-mode hint is suggested when some output is unvalidated.
    #[must_use]
    pub fn with_assurance(mut self, a: openreadout_core::assurance::Assurance) -> Self {
        if a.level != openreadout_core::assurance::AssuranceLevel::Validated {
            self.caveats.insert(
                0,
                format!("Assurance: {}.", a.summary.trim_end_matches('.')),
            );
        }
        self.assurance = Some(a);
        self
    }
}

/// One answer to an `--ask` question, from the experiment model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Answer {
    /// The topic the question matched: `sample`, `channels`, `method`, `run_length`,
    /// `polarity`, `ms_levels`, `instrument`, `operator`, `acquired`, `technique`.
    pub topic: String,
    /// The answer in one or two sentences; says so when the file does not record it.
    pub answer: String,
    /// Paths of the fields the answer came from (into the `info` JSON, or `experiment.*`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
}

/// How many images are described one by one before the rest are summarized.
const MAX_IMAGES_DETAILED: usize = 4;
/// How many table columns are named.
const MAX_COLUMNS_NAMED: usize = 16;

/// Explain `info` in plain English (the experiment is derived from `info` alone).
pub fn explain(info: &FileInfo) -> Explanation {
    explain_with(info, None, None)
}

/// Explain `info` with the dataset's experiment (from [`openreadout_core::experiment::of_dataset`]; derived
/// from `info` when `None`), answering `question` when one is given.
pub fn explain_with(
    info: &FileInfo,
    experiment: Option<&Experiment>,
    question: Option<&str>,
) -> Explanation {
    let derived;
    let exp = if let Some(e) = experiment {
        e
    } else {
        derived = openreadout_core::experiment::derive(
            info,
            &openreadout_core::provenance::ProvenanceMap::new(),
        );
        &derived
    };
    let mut ex = explain_info(info);
    if let Some(p) = experiment_paragraph(exp) {
        ex.paragraphs.insert(0, p);
    }
    let file = quote_path(&info.path);
    if info
        .spectra
        .iter()
        .any(|s| s.ms_levels.iter().any(|l| *l > 1))
    {
        let pos = ex.suggested_commands.len().saturating_sub(2);
        ex.suggested_commands.insert(
            pos,
            format!(
                "openreadout spectra {file} --ms-level 2 --nth 1 --json   # the first MS/MS scan, wherever it falls in the run"
            ),
        );
    }
    if let Some(q) = question {
        ex.answers = answer(info, exp, q);
    }
    ex
}

fn explain_info(info: &FileInfo) -> Explanation {
    let mut ex = Explanation::default();
    let file = quote_path(&info.path);
    let version = info
        .format_version
        .as_deref()
        .map(|v| format!(" (format version {v})"))
        .unwrap_or_default();
    let family = match info.format.family.as_str() {
        "" => String::new(),
        f => format!(" {}", f.replace('-', " ")),
    };
    let what = format!(
        "{} {}{family} file{version}, {}",
        article(&info.format.name),
        info.format.name,
        human_bytes(info.size_bytes)
    );

    let mut holds = Vec::new();
    if !info.images.is_empty() {
        holds.push(count(info.images.len(), "image"));
    }
    if !info.tables.is_empty() {
        holds.push(count(info.tables.len(), "data set"));
    }
    if !info.traces.is_empty() {
        holds.push(count(info.traces.len(), "recording"));
    }
    if !info.spectra.is_empty() {
        holds.push(count(info.spectra.len(), "acquisition run"));
    }
    let holds = if holds.is_empty() {
        "no data the reader could describe".to_string()
    } else {
        join_and(&holds)
    };
    ex.summary = capitalize(&format!("{what}, holding {holds}."));
    if let Some(first) = info.images.first() {
        let same = info.images.iter().all(|i| same_geometry(i, first));
        let lead = match (info.images.len(), same) {
            (1, _) => "It is",
            (_, true) => "Each is",
            _ => "The first is",
        };
        let _ = write!(ex.summary, " {lead} {}", image_phrase(first));
        if let Some(d) = first.acquired_at.as_deref().and_then(date_part) {
            let _ = write!(ex.summary, ", acquired on {d}");
        }
        ex.summary.push('.');
    } else if let Some(t) = info.tables.first() {
        // events × parameters is flow-cytometry wording; other tables have rows and columns
        let (rows, cols) = if info.format.family == "flow-cytometry" {
            ("event", "parameter")
        } else {
            ("row", "column")
        };
        let _ = write!(
            ex.summary,
            " {} {} × {}.",
            if info.tables.len() == 1 {
                "It holds"
            } else {
                "The first holds"
            },
            count_u64(t.row_count, rows),
            count(t.columns.len(), cols)
        );
    }

    if !info.images.is_empty() {
        explain_images(info, &file, &mut ex);
    }
    for t in &info.tables {
        explain_table(info, t, &mut ex);
    }
    for t in &info.traces {
        ex.paragraphs.push(trace_paragraph(t));
    }
    for s in &info.spectra {
        ex.paragraphs.push(spectra_paragraph(s));
    }
    if !info.tables.is_empty() {
        table_commands(info, &file, &mut ex);
    }

    // The analysis that answers the usual question about this kind of data comes first.
    let mut next = crate::explain_next::analysis_commands(info, &file);
    if ex
        .suggested_commands
        .iter()
        .any(|c| c.contains("--per well"))
    {
        next.retain(|c| !c.starts_with("openreadout stats"));
    }
    ex.suggested_commands.splice(0..0, next);
    ex.suggested_commands
        .push(format!("openreadout check {file}"));
    ex.suggested_commands.push(format!(
        "openreadout info {file} --view full --json --no-vendor"
    ));

    for n in &info.notes {
        ex.caveats.push(sentence(n));
    }
    if info.format.confidence != Confidence::High {
        ex.caveats.push(format!(
            "OpenReadout's {} reader has {} confidence{}; `openreadout self formats` lists what it does not handle yet.",
            info.format.name,
            info.format.confidence.as_str(),
            if info.format.known_gaps.is_empty() {
                String::new()
            } else {
                format!(" ({} known gaps)", info.format.known_gaps.len())
            }
        ));
    }
    dedup(&mut ex.caveats);
    dedup(&mut ex.suggested_commands);
    ex
}

// ---------- images ----------

fn explain_images(info: &FileInfo, file: &str, ex: &mut Explanation) {
    let images = &info.images;
    // Contents: one sentence per image, the rest summarized.
    let mut p = String::new();
    if images.len() > 1 {
        let _ = write!(
            p,
            "The file holds {} (scenes, stage positions or series; choose one with `--image N`). ",
            count(images.len(), "image")
        );
    }
    for im in images.iter().take(MAX_IMAGES_DETAILED) {
        p.push_str(&image_sentence(im, images.len() > 1));
        p.push(' ');
    }
    if images.len() > MAX_IMAGES_DETAILED {
        let rest = &images[MAX_IMAGES_DETAILED..];
        let same = rest.iter().all(|i| same_geometry(i, &images[0]));
        let _ = write!(
            p,
            "The remaining {} {}.",
            count(rest.len(), "image"),
            if same {
                "have the same geometry as image 0"
            } else {
                "differ in geometry; `openreadout info` lists each"
            }
        );
    }
    ex.paragraphs.push(p.trim_end().to_string());

    // Channels, per distinct channel set.
    let mut seen: Vec<&[ChannelInfo]> = Vec::new();
    for im in images {
        if seen.contains(&im.channels.as_slice()) {
            continue;
        }
        seen.push(&im.channels);
        if let Some(c) = channels_paragraph(im, images.len() > 1) {
            ex.paragraphs.push(c);
        }
        if seen.len() >= 3 {
            break;
        }
    }

    // Optics and instrument, per distinct setup.
    let mut optics_seen = BTreeSet::new();
    for im in images {
        if let Some(o) = optics_paragraph(im)
            && optics_seen.insert(o.clone())
        {
            ex.paragraphs.push(o);
        }
        if optics_seen.len() >= 3 {
            break;
        }
    }

    if let Some(t) = timing_paragraph(images) {
        ex.paragraphs.push(t);
    }

    let unusual = unusual_paragraph(images);
    if !unusual.is_empty() {
        ex.paragraphs.push(unusual);
    }

    // Caveats specific to images.
    if images.iter().any(|i| i.physical_size.x.is_none()) {
        ex.caveats.push(
            "No pixel size is recorded for some images, so distances cannot be measured in micrometres from them."
                .into(),
        );
    }
    if images.iter().all(|i| i.acquired_at.is_none()) {
        ex.caveats
            .push("The file does not record when the images were acquired.".into());
    }
    if images
        .iter()
        .any(|i| i.acquired_at.as_deref().is_some_and(|a| !has_zone(a)))
        && !info.notes.iter().any(|n| n.contains("time zone"))
    {
        ex.caveats
            .push("Acquisition times are local clock times: the file records no time zone.".into());
    }

    // Commands.
    let big = images.iter().any(|i| {
        i.pyramid_levels > 1 || u64::from(i.size_x) * u64::from(i.size_y) > 64 * 1024 * 1024
    });
    if big {
        ex.suggested_commands.push(format!(
            "openreadout export {file} --format ome-zarr   # large or pyramidal: a chunked, multiscale copy for napari/viewers"
        ));
    }
    // A multi-well plate (one image per field of view): per-well numbers and a plate export.
    let plate = images.len() > 1 && images.iter().all(|i| i.extra.contains_key("well"));
    if plate {
        ex.suggested_commands.push(format!(
            "openreadout stats {file} --per well --select c=0   # per-well intensities of channel 0 (tidy rows; --csv)"
        ));
        ex.suggested_commands.push(format!(
            "openreadout export {file} --format ome-zarr --skip-incomplete   # an OME-NGFF HCS plate (fields whose files are missing left out)"
        ));
    } else {
        ex.suggested_commands.push(format!(
            "openreadout export {file}   # OME-TIFF for Fiji/ImageJ, QuPath, napari, Python"
        ));
    }
    if images.len() > 1 {
        ex.suggested_commands.push(format!(
            "openreadout export {file} --image 1   # only one of the {} images",
            images.len()
        ));
    }
    if let Some(im) = images.iter().find(|i| i.size_c > 1) {
        let name = im
            .channels
            .first()
            .and_then(|c| c.name.as_deref())
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        ex.suggested_commands.push(format!(
            "openreadout export {file} --select c=0 -o {}   # channel 0{name} only",
            quote_path(&sibling(&info.path, ".c0.ome.tiff"))
        ));
    }
    if let Some(im) = images.iter().find(|i| i.size_z > 1) {
        let mid = im.size_z / 2;
        ex.suggested_commands.push(format!(
            "openreadout export {file} --select z={mid} -o {}   # the middle focal plane only",
            quote_path(&sibling(&info.path, &format!(".z{mid}.ome.tiff")))
        ));
    }
    if images
        .iter()
        .any(|i| i.extra.contains_key("frame_records_total"))
    {
        ex.suggested_commands.push(format!(
            "openreadout info {file} --view full --json --no-vendor --all-frames   # per-frame timestamps and stage positions"
        ));
    }
    ex.suggested_commands.push(format!(
        "openreadout check {file} --planes --json   # pixel hashes, to compare copies"
    ));
}

fn same_geometry(a: &ImageInfo, b: &ImageInfo) -> bool {
    (
        a.size_x,
        a.size_y,
        a.size_z,
        a.size_c,
        a.size_t,
        a.pixel_type,
        a.samples_per_pixel,
    ) == (
        b.size_x,
        b.size_y,
        b.size_z,
        b.size_c,
        b.size_t,
        b.pixel_type,
        b.samples_per_pixel,
    )
}

fn is_rgb(im: &ImageInfo) -> bool {
    im.samples_per_pixel >= 3
}

fn is_fluorescence(im: &ImageInfo) -> bool {
    im.channels.iter().any(|c| {
        c.fluorophore.is_some()
            || c.excitation_nm.is_some()
            || c.acquisition_mode.as_deref().is_some_and(|m| {
                let m = m.to_ascii_lowercase();
                m.contains("fluor") || m.contains("confocal") || m.contains("photon")
            })
    })
}

/// "a 5-channel fluorescence z-stack (21 slices) of 1019 × 1019 pixels at 1.02 µm per pixel"
fn image_phrase(im: &ImageInfo) -> String {
    let mut kind = Vec::new();
    if is_rgb(im) {
        kind.push("colour (RGB)".to_string());
    } else if im.size_c > 1 {
        kind.push(format!("{}-channel", im.size_c));
    }
    if !is_rgb(im) && is_fluorescence(im) {
        kind.push("fluorescence".into());
    }
    let shape = match (im.size_z > 1, im.size_t > 1, im.size_y == 1) {
        (_, _, true) => "line scan".to_string(),
        (true, true, _) => format!(
            "time-lapse z-stack ({} slices × {} time points)",
            im.size_z, im.size_t
        ),
        (true, false, _) => format!("z-stack ({} slices)", im.size_z),
        (false, true, _) => format!("time series ({} time points)", im.size_t),
        (false, false, _) => "image".into(),
    };
    kind.push(shape);
    let kind = kind.join(" ");
    let mut s = format!(
        "{} {kind} of {} × {} pixels",
        article(&kind),
        im.size_x,
        im.size_y
    );
    if let Some(x) = im.physical_size.x {
        let _ = write!(s, " at {} per pixel", fmt_um(x));
    }
    s
}

/// One sentence about one image's geometry, calibration and storage.
fn image_sentence(im: &ImageInfo, numbered: bool) -> String {
    let mut s = if numbered {
        format!(
            "Image {}{}: ",
            im.index,
            im.name
                .as_deref()
                .map(|n| format!(" (\u{201c}{n}\u{201d})"))
                .unwrap_or_default()
        )
    } else {
        "The image is ".to_string()
    };
    let _ = write!(s, "{} × {} pixels", im.size_x, im.size_y);
    if let (Some(px), Some(py)) = (
        im.physical_size.x,
        im.physical_size.y.or(im.physical_size.x),
    ) {
        let _ = write!(
            s,
            ", a field of view of {} × {} at {} per pixel",
            fmt_len(f64::from(im.size_x) * px),
            fmt_len(f64::from(im.size_y) * py),
            fmt_um(px)
        );
    }
    if im.size_z > 1 {
        let _ = write!(s, "; {} focal planes", im.size_z);
        if let Some(z) = im.physical_size.z {
            let _ = write!(
                s,
                " {} apart (a depth of {})",
                fmt_um(z),
                fmt_len(z * f64::from(im.size_z - 1))
            );
        }
    }
    if im.size_t > 1 {
        let _ = write!(s, "; {} time points", im.size_t);
        if let Some(dt) = im.time_increment_s.filter(|d| *d > 0.0) {
            let _ = write!(s, " every {}", fmt_duration(dt));
        }
    }
    if !is_rgb(im) && im.size_c > 1 {
        let _ = write!(s, "; {} channels", im.size_c);
    }
    let _ = write!(s, "; {}", sample_words(im));
    let _ = write!(s, "; {} in total.", count_u64(im.plane_count, "plane"));
    s
}

fn sample_words(im: &ImageInfo) -> String {
    let bits = im.pixel_type.bytes_per_sample() * 8;
    let t = match im.pixel_type {
        PixelType::Int8 | PixelType::Int16 | PixelType::Int32 | PixelType::Int64 => {
            format!("signed {bits}-bit integers")
        }
        PixelType::Uint8 => "8-bit values (0 to 255)".into(),
        PixelType::Uint16 => "16-bit values (0 to 65535)".into(),
        PixelType::Uint32 => "32-bit unsigned integers".into(),
        PixelType::Uint64 => "64-bit unsigned integers".into(),
        PixelType::Float | PixelType::Double => format!("{bits}-bit floating-point values"),
        PixelType::ComplexFloat | PixelType::ComplexDouble => format!(
            "complex numbers ({}-bit real and imaginary parts; statistics and previews use the amplitude)",
            bits / 2
        ),
        other => format!("{} values", other.ome_name()),
    };
    let used = im
        .extra
        .get("bits_significant")
        .or_else(|| im.extra.get("component_bit_count"))
        .and_then(Value::as_u64)
        .filter(|b| *b > 0 && (*b as usize) < bits)
        .map(|b| format!(", of which the camera filled {b} bits"))
        .unwrap_or_default();
    if is_rgb(im) {
        format!("red, green and blue {t} per pixel{used}")
    } else {
        format!("pixels are {t}{used}")
    }
}

fn channels_paragraph(im: &ImageInfo, numbered: bool) -> Option<String> {
    let lead = if numbered {
        format!("Channels of image {}: ", im.index)
    } else {
        "Channels: ".to_string()
    };
    if is_rgb(im) {
        let modes = modes(im);
        return Some(format!(
            "{lead}each pixel holds red, green and blue values, as a colour camera records them (typical of brightfield, stained tissue or slide scans){}; there are no fluorescence wavelengths to report.",
            if modes.is_empty() {
                String::new()
            } else {
                format!(", acquired as {}", modes.join(", "))
            }
        ));
    }
    if im.channels.is_empty() {
        return None;
    }
    let parts: Vec<String> = im.channels.iter().map(channel_words).collect();
    let mut s = format!("{lead}{}.", parts.join("; "));
    if im.channels.iter().all(|c| {
        c.excitation_nm.is_none()
            && c.emission_nm.is_none()
            && c.emission_range_nm.is_none()
            && c.fluorophore.is_none()
    }) {
        s.push_str(" No dyes or wavelengths are recorded.");
    }
    Some(s)
}

fn channel_words(c: &ChannelInfo) -> String {
    let mut s = format!("{}", c.index);
    if let Some(n) = &c.name {
        let _ = write!(s, " \u{201c}{n}\u{201d}");
    }
    let mut d = Vec::new();
    if let Some(f) = &c.fluorophore {
        d.push(f.clone());
    }
    if let Some(e) = c.excitation_nm {
        d.push(format!("excited at {e:.0} nm"));
    }
    if let Some(e) = c.emission_nm {
        d.push(format!("{} light at {e:.0} nm", colour_name(e)));
    } else if let Some([a, b]) = c.emission_range_nm {
        d.push(format!(
            "{} light detected at {a:.0}–{b:.0} nm",
            colour_name(f64::midpoint(a, b))
        ));
    }
    if let Some(m) = &c.acquisition_mode {
        d.push(m.to_lowercase());
    }
    if let Some(e) = c.exposure_ms {
        d.push(format!("{} exposure", fmt_duration(e / 1000.0)));
    }
    if let Some(col) = &c.color {
        d.push(format!("shown in {}", display_colour(col)));
    }
    if !d.is_empty() {
        let _ = write!(s, " ({})", d.join(", "));
    }
    s
}

/// Everyday colour of a wavelength (for the emission of a dye).
fn colour_name(nm: f64) -> &'static str {
    match nm {
        x if x < 400.0 => "ultraviolet",
        x if x < 450.0 => "violet",
        x if x < 495.0 => "blue",
        x if x < 570.0 => "green",
        x if x < 590.0 => "yellow",
        x if x < 620.0 => "orange",
        x if x < 700.0 => "red",
        x if x < 780.0 => "far red",
        _ => "near infrared",
    }
}

/// `#RRGGBB` as a colour word when it is a pure display colour, else the hex code.
fn display_colour(hex: &str) -> String {
    match hex.to_ascii_uppercase().as_str() {
        "#FF0000" => "red",
        "#00FF00" => "green",
        "#0000FF" => "blue",
        "#FFFFFF" => "white (grey scale)",
        "#FF00FF" => "magenta",
        "#00FFFF" => "cyan",
        "#FFFF00" => "yellow",
        "#FF8000" | "#FFA500" => "orange",
        _ => return hex.to_string(),
    }
    .to_string()
}

fn modes(im: &ImageInfo) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for m in im
        .channels
        .iter()
        .filter_map(|c| c.acquisition_mode.as_deref())
    {
        let m = m.to_lowercase();
        if !v.contains(&m) {
            v.push(m);
        }
    }
    v
}

fn optics_paragraph(im: &ImageInfo) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(o) = &im.objective {
        let mut d = Vec::new();
        if let Some(m) = o.nominal_magnification {
            d.push(format!("{}× magnification", trim_num(m)));
        }
        if let Some(na) = o.lens_na {
            d.push(format!("numerical aperture {}", trim_num(na)));
        }
        if let Some(i) = &o.immersion {
            d.push(format!("{} immersion", immersion_word(i)));
        }
        let name = o
            .model
            .as_deref()
            .map(clean)
            .filter(|m| !m.is_empty())
            .map_or_else(
                || "an objective".to_string(),
                |m| format!("{} {m} objective", article(&m)),
            );
        parts.push(if d.is_empty() {
            format!("Imaged through {name}")
        } else {
            format!("Imaged through {name} ({})", d.join(", "))
        });
    }
    if let Some(i) = &im.instrument {
        let scope: Vec<String> = [i.manufacturer.as_deref(), i.model.as_deref()]
            .into_iter()
            .flatten()
            .map(clean)
            .filter(|s| !s.is_empty())
            .collect();
        if !scope.is_empty() {
            let scope = scope.join(" ");
            parts.push(format!("on {} {scope} instrument", article(&scope)));
        }
        if let Some(d) = &i.detector {
            parts.push(format!("with the detector {}", clean(d)));
        }
        if let Some(sw) = &i.software {
            let v = i
                .software_version
                .as_deref()
                .map(|v| format!(" {}", clean(v)))
                .unwrap_or_default();
            parts.push(format!("recorded by {}{v}", clean(sw)));
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut s = parts.join(", ");
    if !s.starts_with("Imaged") {
        s = format!("Acquired {s}");
    }
    s.push('.');
    Some(s)
}

/// Collapse runs of whitespace (vendor strings are often padded).
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn immersion_word(s: &str) -> String {
    let l = s.trim().to_ascii_lowercase();
    match l.as_str() {
        "glyc" | "glycerin" | "glycerine" | "glycerol" => "glycerol".into(),
        "dry" | "air" => "air (dry)".into(),
        "w" | "water" => "water".into(),
        "oel" | "oil" => "oil".into(),
        _ => l,
    }
}

fn timing_paragraph(images: &[ImageInfo]) -> Option<String> {
    let mut parts = Vec::new();
    let starts: Vec<&str> = images
        .iter()
        .filter_map(|i| i.acquired_at.as_deref())
        .collect();
    if let Some(first) = starts.iter().min() {
        let last = starts.iter().max().copied().unwrap_or(first);
        let zone = if has_zone(first) {
            " (UTC)"
        } else {
            " (local time)"
        };
        if first == &last || starts.len() == 1 {
            parts.push(format!(
                "Acquisition started {}{zone}.",
                readable_time(first)
            ));
        } else {
            parts.push(format!(
                "Acquisition started {}{zone}; the last image started {}.",
                readable_time(first),
                readable_time(last)
            ));
        }
    }
    let records: u64 = images
        .iter()
        .filter_map(|i| i.extra.get("frame_records_total").and_then(Value::as_u64))
        .sum();
    if records > 0 {
        parts.push(format!(
            "The file keeps a record for each of its {} (acquisition time, stage position and similar; `info --view full --all-frames` lists them).",
            count_u64(records, "frame")
        ));
    }
    for im in images.iter().filter(|i| i.size_t > 1).take(1) {
        match im.time_increment_s.filter(|d| *d > 0.0) {
            Some(dt) => parts.push(format!(
                "The time series has {} time points taken every {}, covering about {}.",
                im.size_t,
                fmt_duration(dt),
                fmt_duration(dt * f64::from(im.size_t - 1))
            )),
            None => parts.push(format!(
                "The time series has {} time points; the interval is not recorded in the header (per-frame times, when the format has them, are in `info --view full`).",
                im.size_t
            )),
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn unusual_paragraph(images: &[ImageInfo]) -> String {
    let mut v = Vec::new();
    // Mosaics, grouped by tile layout.
    let mut groups: Vec<(&openreadout_core::model::MosaicInfo, Vec<u32>)> = Vec::new();
    for im in images {
        if let Some(m) = &im.mosaic {
            match groups.iter_mut().find(|(g, _)| *g == m) {
                Some((_, ids)) => ids.push(im.index),
                None => groups.push((m, vec![im.index])),
            }
        }
    }
    for (m, ids) in groups.iter().take(3) {
        let tile = match (m.tile_width, m.tile_height) {
            (Some(w), Some(h)) => format!(" of {w} × {h} pixels"),
            _ => String::new(),
        };
        let who = if ids.len() == 1 {
            format!("image {} is", ids[0])
        } else {
            let list: Vec<String> = ids.iter().take(6).map(u32::to_string).collect();
            format!(
                "images {}{} are each",
                join_and(&list),
                if ids.len() > 6 { " (and more)" } else { "" }
            )
        };
        v.push(format!(
            "{who} a mosaic of {} tiles{tile}{}",
            m.tile_count,
            if m.stitched_on_read {
                ", stitched into one picture when read"
            } else {
                ", each tile exposed as its own image"
            }
        ));
    }
    if let Some(im) = images.iter().find(|i| i.pyramid_levels > 1) {
        v.push(format!(
            "image {} also stores {} (a pyramid, as slide scanners write); exports use full resolution",
            im.index,
            count(im.pyramid_levels as usize - 1, "lower-resolution copy")
        ));
    }
    if let Some(im) = images.iter().find(|i| i.size_y == 1) {
        v.push(format!(
            "image {} is a line scan: one line of pixels repeated over time, not a 2-D picture",
            im.index
        ));
    }
    let events: u64 = images
        .iter()
        .filter_map(|i| i.extra.get("event_count").and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    if events > 0 {
        v.push(format!(
            "the acquisition log records {} (stimulations, pauses, commands; see `extra.events`)",
            count_u64(events, "event")
        ));
    }
    let rois = images
        .iter()
        .filter_map(|i| i.extra.get("rois").and_then(Value::as_array))
        .map(Vec::len)
        .sum::<usize>();
    if rois > 0 {
        v.push(format!(
            "{} drawn during acquisition {} stored",
            count(rois, "region of interest"),
            if rois == 1 { "is" } else { "are" }
        ));
    }
    let area = |i: &&ImageInfo| u64::from(i.size_x) * u64::from(i.size_y);
    if let (Some(small), Some(large)) = (
        images.iter().min_by_key(area),
        images.iter().max_by_key(area),
    ) && area(&small) * 4 <= area(&large)
    {
        v.push(format!(
            "the images differ a lot in size, from {} × {} to {} × {} pixels (for example an overview plus detail scans)",
            small.size_x, small.size_y, large.size_x, large.size_y
        ));
    }
    if v.is_empty() {
        return String::new();
    }
    let mut s = "Worth knowing: ".to_string();
    s.push_str(&v.join("; "));
    s.push('.');
    s
}

// ---------- tables (flow cytometry) ----------

fn extra_str<'a>(t: &'a TableInfo, k: &str) -> Option<&'a str> {
    t.extra
        .get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

#[allow(clippy::many_single_char_names)]
fn explain_table(info: &FileInfo, t: &TableInfo, ex: &mut Explanation) {
    let flow = info.format.family == "flow-cytometry";
    let mass = t
        .extra
        .get("platform")
        .and_then(|p| p.get("technology"))
        .and_then(Value::as_str)
        == Some("mass");
    let rows = if flow {
        count_u64(t.row_count, "event")
    } else {
        count_u64(t.row_count, "row")
    };
    let mut s = format!(
        "Data set {}{} holds {rows}",
        t.index,
        t.name
            .as_deref()
            .map(|n| format!(" (\u{201c}{n}\u{201d})"))
            .unwrap_or_default()
    );
    if mass {
        s.push_str(" (cells or particles that reached the mass spectrometer)");
    } else if flow {
        s.push_str(" (cells or particles that passed the laser)");
    }
    let _ = write!(
        s,
        ", each measured on {}",
        count(t.columns.len(), if flow { "parameter" } else { "column" })
    );
    let named: Vec<String> = t
        .columns
        .iter()
        .take(MAX_COLUMNS_NAMED)
        .map(|c| match &c.label {
            Some(l) if !l.trim().is_empty() && l != &c.name => format!("{} ({l})", c.name),
            _ => c.name.clone(),
        })
        .collect();
    if !named.is_empty() {
        let _ = write!(s, ": {}", named.join(", "));
        if t.columns.len() > MAX_COLUMNS_NAMED {
            let _ = write!(s, " and {} more", t.columns.len() - MAX_COLUMNS_NAMED);
        }
    }
    s.push('.');
    if flow {
        let kinds = flow_parameter_kinds(t);
        if !kinds.is_empty() {
            let _ = write!(s, " {kinds}");
        }
    }
    ex.paragraphs.push(s);

    let mut p = Vec::new();
    let model = t
        .extra
        .get("instrument")
        .and_then(|i| i.get("model"))
        .and_then(Value::as_str);
    let vendor = t
        .extra
        .get("platform")
        .and_then(|p| p.get("vendor"))
        .and_then(Value::as_str);
    match (model, vendor) {
        (Some(m), Some(v)) => p.push(format!("measured on the cytometer {m} ({v})")),
        (Some(m), None) => p.push(format!("measured on the cytometer {m}")),
        (None, Some(v)) => p.push(format!("measured on a {v} cytometer")),
        (None, None) => {}
    }
    if let Some(sw) = extra_str(t, "software") {
        p.push(format!("recorded by {sw}"));
    }
    match (
        extra_str(t, "acquisition_start"),
        extra_str(t, "acquisition_end"),
    ) {
        (Some(a), Some(b)) if a != b => p.push(format!(
            "acquired from {} to {}{}",
            readable_time(a),
            readable_time(b),
            if has_zone(a) { " UTC" } else { " (local time)" }
        )),
        (Some(a), _) => p.push(format!(
            "acquired {}{}",
            readable_time(a),
            if has_zone(a) { " UTC" } else { " (local time)" }
        )),
        _ => {
            if let Some(d) = extra_str(t, "acquisition_date") {
                p.push(format!("acquired on {d}"));
            }
        }
    }
    for (k, w) in [
        ("specimen", "specimen"),
        ("cells", "cells"),
        ("experimenter", "experimenter"),
        ("operator", "operator"),
        ("institution", "institution"),
    ] {
        if let Some(v) = extra_str(t, k) {
            p.push(format!("{w}: {v}"));
        }
    }
    if let Some(v) = t.extra.get("volume_nl").and_then(Value::as_f64) {
        p.push(format!("{} of sample analysed", fmt_volume_nl(v)));
    }
    if !p.is_empty() {
        ex.paragraphs
            .push(format!("Data set {}: {}.", t.index, p.join("; ")));
    }
    if let Some(pl) = t.extra.get("platform") {
        platform_paragraph(t, pl, ex);
    }
    if let Some(sp) = t.extra.get("spillover") {
        let n = sp
            .get("parameters")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let kw = sp.get("keyword").and_then(Value::as_str).unwrap_or("?");
        ex.paragraphs.push(format!(
            "{} {n} × {n} compensation (spillover) matrix is stored (keyword {kw}): it says how much of each fluorochrome's light leaks into the other detectors. OpenReadout returns raw values unless asked: `openreadout table FILE --compensate` applies it.",
            capitalize(article(&n.to_string()))
        ));
    } else if flow && !mass {
        ex.caveats.push(format!(
            "Data set {} carries no compensation (spillover) matrix; compensate from single-stain controls if the dyes overlap.",
            t.index
        ));
    }
    if flow && !mass {
        ex.caveats.push(
            "Event values are raw detector readings: not compensated and not transformed (log/logicle) for display; `openreadout table FILE --compensate --transform logicle` does both, and `openreadout analyze gate FILE --workspace W.wsp` counts FlowJo populations.".into(),
        );
    }
    if extra_str(t, "acquisition_start").is_some_and(|a| !has_zone(a))
        && !info.notes.iter().any(|n| n.contains("time zone"))
    {
        ex.caveats
            .push("FCS times are local clock times: the file records no time zone.".into());
    }
}

/// What the file's instrument family means for the reader: spectral detectors, metal tags.
fn platform_paragraph(t: &TableInfo, pl: &Value, ex: &mut Explanation) {
    let s = |k: &str| pl.get(k).and_then(Value::as_str);
    match s("technology") {
        Some("spectral") => {
            let mut p = format!(
                "Data set {} comes from a full-spectrum (spectral) cytometer{}",
                t.index,
                s("vendor").map(|v| format!(" by {v}")).unwrap_or_default()
            );
            if let Some(n) = pl.get("detector_count").and_then(Value::as_u64) {
                let lasers: Vec<String> = pl
                    .get("detectors_by_laser")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(l, d)| format!("{l} {}", d.as_array().map_or(0, Vec::len)))
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = write!(p, ": each event is measured on {n} detectors");
                if !lasers.is_empty() {
                    let _ = write!(p, " across the lasers ({})", lasers.join(", "));
                }
            }
            p.push_str(". Fluorochrome abundances come from unmixing those spectra against single-stain reference controls");
            match pl
                .get("unmixing")
                .and_then(|u| u.get("stored"))
                .and_then(Value::as_bool)
            {
                Some(false) => {
                    p.push_str("; this file holds the raw detector signals, not unmixed values.");
                }
                Some(true) => p.push_str("; this file already holds unmixed values."),
                None => p.push('.'),
            }
            ex.paragraphs.push(p);
        }
        Some("mass") => {
            let n = pl
                .get("mass_channel_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let markers: Vec<String> = pl
                .get("markers_by_isotope")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(iso, mk)| mk.as_str().map(|mk| format!("{mk} ({iso})")))
                        .take(10)
                        .collect()
                })
                .unwrap_or_default();
            let mut p = format!(
                "Data set {} is mass cytometry (CyTOF): antibodies carry metal isotopes, and each event's metals are counted by time-of-flight mass spectrometry. It has {}",
                t.index,
                count(usize::try_from(n).unwrap_or(0), "metal channel")
            );
            if !markers.is_empty() {
                let _ = write!(p, "; markers include {}", markers.join(", "));
            }
            p.push('.');
            ex.paragraphs.push(p);
            ex.caveats.push(
                "Mass-cytometry values are ion counts; analyses usually take arcsinh(x / 5) first (`openreadout table FILE --transform arcsinh-cofactor:5`).".into(),
            );
        }
        _ => {}
    }
}

/// Scatter vs fluorescence vs time columns: by the reader's `channel_kind` when it gives one,
/// else by the conventional FCS names.
fn flow_parameter_kinds(t: &TableInfo) -> String {
    let kinds: Vec<&str> = t
        .columns
        .iter()
        .filter_map(|c| c.extra.get("channel_kind").and_then(Value::as_str))
        .collect();
    if !kinds.is_empty() && kinds.len() == t.columns.len() {
        let n = |k: &str| kinds.iter().filter(|x| **x == k).count();
        let mut v = Vec::new();
        for (k, what) in [
            ("scatter", "scatter (cell size and granularity)"),
            ("fluorescence", "fluorescence"),
            ("spectral_detector", "full-spectrum detector"),
            ("unmixed_fluorescence", "unmixed fluorescence"),
            ("imaging_feature", "image-derived feature"),
            ("mass", "metal-isotope (mass)"),
            ("gaussian_parameter", "pulse-shape (Gaussian) parameter"),
            ("event_length", "event length"),
            ("background", "background"),
            ("other", "other"),
        ] {
            let c = n(k);
            if c > 0 {
                v.push(format!("{c} {what}"));
            }
        }
        if n("time") > 0 {
            v.push("a time stamp".to_string());
        }
        return format!("By role, these are {}.", join_and(&v));
    }
    let up = |c: &openreadout_core::model::ColumnInfo| c.name.to_ascii_uppercase();
    let scatter = t
        .columns
        .iter()
        .filter(|c| {
            let n = up(c);
            n.starts_with("FSC")
                || n.starts_with("SSC")
                || n.starts_with("FS ")
                || n.starts_with("SS ")
        })
        .count();
    let time = t
        .columns
        .iter()
        .any(|c| up(c) == "TIME" || up(c).starts_with("TIME"));
    let fl = t.columns.len() - scatter - usize::from(time);
    let mut v = Vec::new();
    if scatter > 0 {
        v.push(format!("{scatter} scatter (cell size and granularity)"));
    }
    if fl > 0 && scatter > 0 {
        v.push(format!("{fl} fluorescence or other"));
    }
    if time {
        v.push("a time stamp".to_string());
    }
    if v.is_empty() {
        String::new()
    } else {
        format!("By name, these are {}.", join_and(&v))
    }
}

fn table_commands(info: &FileInfo, file: &str, ex: &mut Explanation) {
    ex.suggested_commands.push(format!(
        "openreadout export {file} --format csv --labels   # events as a spreadsheet (one row per event)"
    ));
    if info.tables.len() > 1 {
        ex.suggested_commands.push(format!(
            "openreadout export {file} --format csv --table 1   # the second of {} data sets",
            info.tables.len()
        ));
    }
    if info.tables.iter().any(|t| t.row_count > 100_000) {
        ex.suggested_commands.push(format!(
            "openreadout export {file} --format csv --rows 0-9999   # the first 10,000 events only"
        ));
    }
    if info.format.family == "flow-cytometry" {
        if info
            .tables
            .iter()
            .any(|t| t.extra.contains_key("spillover"))
        {
            ex.suggested_commands.push(format!(
                "openreadout table {file} --compensate --transform logicle --json   # compensated, logicle-scaled events"
            ));
        }
        ex.suggested_commands.push(format!(
            "openreadout analyze gate {file} --workspace analysis.wsp   # population counts from a FlowJo workspace (or --gatingml)"
        ));
    }
}

// ---------- traces and spectra ----------

fn trace_paragraph(t: &TraceInfo) -> String {
    let mut s = format!(
        "Recording {}{}: {}",
        t.index,
        t.name
            .as_deref()
            .map(|n| format!(" (\u{201c}{n}\u{201d})"))
            .unwrap_or_default(),
        count(t.channels.len(), "channel")
    );
    let ch: Vec<String> = t
        .channels
        .iter()
        .take(8)
        .map(|c| match &c.unit {
            Some(u) => format!("{} in {u}", c.name),
            None => c.name.clone(),
        })
        .collect();
    if !ch.is_empty() {
        let _ = write!(s, " ({})", ch.join(", "));
    }
    if t.sample_rate_hz > 0.0 {
        let secs = t.sample_count as f64 / t.sample_rate_hz;
        let _ = write!(
            s,
            " sampled at {}; {} of {} samples ({})",
            fmt_rate(t.sample_rate_hz),
            if t.sweep_count > 1 {
                format!("{} sweeps", t.sweep_count)
            } else {
                "one continuous stretch".to_string()
            },
            fmt_count(t.sample_count),
            fmt_duration(secs)
        );
        if t.sweep_count > 1 {
            s.push_str(" each");
        }
    }
    s.push('.');
    s
}

fn spectra_paragraph(r: &SpectraInfo) -> String {
    let mut s = format!(
        "Run {}{}: {}",
        r.index,
        r.name
            .as_deref()
            .map(|n| format!(" (\u{201c}{n}\u{201d})"))
            .unwrap_or_default(),
        count_u64(r.scan_count, "mass spectrum")
    );
    if !r.ms_levels.is_empty() {
        let lv: Vec<String> = r.ms_levels.iter().map(|l| format!("MS{l}")).collect();
        let _ = write!(
            s,
            " at level{} {}",
            if lv.len() > 1 { "s" } else { "" },
            lv.join(" and ")
        );
        if r.ms_levels.iter().any(|l| *l > 1) {
            s.push_str(" (MS2 and above are fragment spectra of selected ions)");
        }
    }
    if let Some([a, b]) = r.rt_range_s {
        let _ = write!(
            s,
            ", over {} to {} of retention time",
            fmt_duration(a),
            fmt_duration(b)
        );
    }
    if let Some(i) = &r.instrument {
        let who: Vec<&str> = [i.manufacturer.as_deref(), i.model.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !who.is_empty() {
            let _ = write!(s, ", on a {}", who.join(" "));
        }
    }
    s.push('.');
    s
}

// ---------- experiment ----------

fn value_words(v: &Value) -> Option<String> {
    Some(match v {
        Value::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => fmt_count(u),
            (_, Some(i)) => i.to_string(),
            _ => sig3(n.as_f64()?),
        },
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        Value::Array(a) => {
            let items: Option<Vec<String>> = a
                .iter()
                .map(|x| match x {
                    Value::Object(_) | Value::Array(_) => None,
                    _ => value_words(x),
                })
                .collect();
            let items = items?;
            if items.is_empty() {
                return None;
            }
            items.join(", ")
        }
        _ => return None,
    })
}

fn quantity_words(q: &Quantity) -> Option<String> {
    let v = value_words(&q.value)?;
    Some(match &q.unit {
        Some(u) => format!("{v} {u}"),
        None => v,
    })
}

fn exp_sample_words(e: &Experiment) -> Option<String> {
    let s = e.sample.as_ref()?;
    let mut p = String::new();
    if let Some(id) = &s.id {
        let _ = write!(p, "sample \u{201c}{id}\u{201d}");
        if let Some(f) = &s.source_field {
            let _ = write!(p, " (from `{f}`)");
        }
    }
    if let Some(n) = &s.name {
        if p.is_empty() {
            let _ = write!(p, "sample \u{201c}{n}\u{201d}");
        } else {
            let _ = write!(p, ", named \u{201c}{n}\u{201d}");
        }
    }
    let mut more = Vec::new();
    if let Some(w) = &s.well {
        more.push(format!("well {w}"));
    }
    if let Some(b) = s.barcode.as_ref().filter(|b| s.id.as_ref() != Some(*b)) {
        more.push(format!("barcode {b}"));
    }
    if let Some(v) = &s.sequence_position {
        more.push(format!("autosampler/sequence position {v}"));
    }
    if !more.is_empty() {
        if p.is_empty() {
            p = format!("sample at {}", more.join(", "));
        } else {
            let _ = write!(p, ", {}", more.join(", "));
        }
    }
    (!p.is_empty()).then_some(p)
}

fn method_words(e: &Experiment) -> Option<String> {
    let m = e.method.as_ref()?;
    let mut p = String::new();
    if let Some(t) = &m.technique {
        let _ = write!(p, "measured by {} ({})", t.label, t.id);
    }
    if let Some(n) = &m.name {
        let _ = write!(
            p,
            "{}method \u{201c}{n}\u{201d}",
            if p.is_empty() { "" } else { " with " }
        );
    }
    let params: Vec<String> = m
        .parameters
        .iter()
        .filter(|(k, _)| k.as_str() != "channels")
        .filter_map(|(k, q)| quantity_words(q).map(|w| format!("{} {w}", k.replace('_', " "))))
        .take(12)
        .collect();
    if !params.is_empty() {
        if p.is_empty() {
            p.push_str("method settings");
        }
        let _ = write!(p, " ({})", params.join(", "));
    }
    (!p.is_empty()).then_some(p)
}

/// "Experiment: sample …; measured by … with method … (…); operator …; 19 min of data."
fn experiment_paragraph(e: &Experiment) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(exp_sample_words(e));
    parts.extend(method_words(e));
    if let Some(a) = &e.acquisition {
        if let Some(o) = &a.operator {
            parts.push(format!("operator {o}"));
        }
        if let Some(d) = a.duration_s {
            parts.push(format!("{} of data", fmt_duration(d)));
        }
        if let Some(t) = &a.comment {
            parts.push(format!("comment \u{201c}{t}\u{201d}"));
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut s = format!("Experiment: {}.", parts.join("; "));
    for n in &e.notes {
        s.push(' ');
        s.push_str(n);
    }
    Some(s)
}

/// Question topics and the words that select them.
const TOPICS: &[(&str, &[&str])] = &[
    (
        "sample",
        &[
            "sample",
            "specimen",
            "well",
            "barcode",
            "vial",
            "tube",
            "which plate",
            "position",
        ],
    ),
    (
        "channels",
        &[
            "channel",
            "dye",
            "fluoro",
            "stain",
            "dapi",
            "gfp",
            "marker",
            "wavelength",
            "excitation",
            "emission",
            "filter",
            "detector",
        ],
    ),
    (
        "method",
        &[
            "gradient",
            "oven",
            "temperature program",
            "solvent",
            "column",
            "method",
            "protocol",
            "pulse program",
            "pulse sequence",
            "settings",
            "parameter",
            "conditions",
        ],
    ),
    (
        "run_length",
        &[
            "how long",
            "duration",
            "run length",
            "run time",
            "length of",
            "long was",
            "long is",
        ],
    ),
    (
        "polarity",
        &[
            "polarity",
            "positive",
            "negative",
            "ion mode",
            "ionization mode",
            "ionisation mode",
        ],
    ),
    (
        "ms_levels",
        &[
            "ms level",
            "ms/ms",
            "msms",
            "ms2",
            "ms1",
            "tandem",
            "fragment",
            "precursor",
        ],
    ),
    (
        "instrument",
        &[
            "instrument",
            "machine",
            "microscope",
            "spectrometer",
            "cytometer",
            "reader",
            "model",
            "serial",
            "software",
        ],
    ),
    (
        "operator",
        &["who ", "operator", "user", "acquired by", "scientist"],
    ),
    (
        "acquired",
        &["when", "date", "which day", "started", "acquired on"],
    ),
    ("comment", &["comment", "note", "remark", "annotation"]),
    (
        "technique",
        &[
            "technique",
            "what was measured",
            "what kind",
            "assay",
            "modality",
            "what type",
        ],
    ),
];

/// Answer a plain-words question from the experiment model and `info`.
fn answer(info: &FileInfo, e: &Experiment, question: &str) -> Vec<Answer> {
    let q = format!("{} ", question.to_lowercase());
    let mut topics: Vec<&str> = TOPICS
        .iter()
        .filter(|(_, words)| words.iter().any(|w| q.contains(w)))
        .map(|(t, _)| *t)
        .collect();
    if topics.is_empty() {
        topics = vec!["technique", "sample", "method", "run_length"];
    }
    let gradient = ["gradient", "oven", "temperature program", "solvent"]
        .iter()
        .any(|w| q.contains(w));
    topics
        .into_iter()
        .map(|t| match t {
            "sample" => answer_sample(e),
            "channels" => answer_channels(info),
            "method" => answer_method(info, e, gradient),
            "run_length" => answer_run_length(info, e),
            "polarity" => answer_polarity(e),
            "ms_levels" => answer_ms_levels(info),
            "instrument" => answer_instrument(e),
            "operator" => simple(
                "operator",
                e.acquisition.as_ref().and_then(|a| a.operator.as_deref()),
                |o| format!("The file records the operator (user) as \u{201c}{o}\u{201d}."),
                "The file does not record who acquired it.",
                e.origin_of("acquisition.operator").map(|o| o.from.clone()),
            ),
            "acquired" => answer_acquired(e),
            "comment" => simple(
                "comment",
                e.acquisition.as_ref().and_then(|a| a.comment.as_deref()),
                |t| format!("The file's comment reads \u{201c}{t}\u{201d}."),
                "The file records no comment.",
                e.origin_of("acquisition.comment").map(|o| o.from.clone()),
            ),
            _ => answer_technique(e),
        })
        .collect()
}

fn simple(
    topic: &str,
    value: Option<&str>,
    yes: impl FnOnce(&str) -> String,
    no: &str,
    from: Option<String>,
) -> Answer {
    Answer {
        topic: topic.into(),
        answer: value.map_or_else(|| no.to_string(), yes),
        fields: from.into_iter().collect(),
    }
}

fn answer_sample(e: &Experiment) -> Answer {
    let fields = [
        "sample.id",
        "sample.name",
        "sample.well",
        "sample.barcode",
        "sample.sequence_position",
    ]
    .iter()
    .filter_map(|p| e.origin_of(p).map(|o| o.from.clone()))
    .collect();
    Answer {
        topic: "sample".into(),
        answer: exp_sample_words(e).map_or_else(
            || "The file does not record a sample identifier, name, well or barcode.".into(),
            |s| format!("{}.", capitalize(&s)),
        ),
        fields,
    }
}

fn answer_channels(info: &FileInfo) -> Answer {
    let mut fields = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    if let Some(im) = info.images.iter().find(|i| !i.channels.is_empty()) {
        fields.push(format!("images[{}].channels", im.index));
        for c in &im.channels {
            let mut d = Vec::new();
            if let Some(f) = &c.fluorophore {
                d.push(format!("dye {f}"));
            }
            if let Some(x) = c.excitation_nm {
                d.push(format!("excitation {x:.0} nm"));
            }
            if let Some(x) = c.emission_nm {
                d.push(format!("emission {x:.0} nm"));
            } else if let Some([a, b]) = c.emission_range_nm {
                d.push(format!("detection {a:.0}–{b:.0} nm"));
            }
            if let Some(m) = &c.acquisition_mode {
                d.push(m.to_lowercase());
            }
            let name = c
                .name
                .as_deref()
                .map(|n| format!(" \u{201c}{n}\u{201d}"))
                .unwrap_or_default();
            parts.push(if d.is_empty() {
                format!("channel {}{name}", c.index)
            } else {
                format!("channel {}{name} ({})", c.index, d.join(", "))
            });
        }
        if info.images.iter().any(|i| i.channels != im.channels) {
            parts.push("other images have different channels (see `info`)".into());
        }
    } else if let Some(t) = info.tables.first() {
        let labelled: Vec<String> = t
            .columns
            .iter()
            .filter_map(|c| {
                c.label
                    .as_deref()
                    .filter(|l| !l.trim().is_empty() && *l != c.name)
                    .map(|l| format!("{} is \u{201c}{l}\u{201d}", c.name))
            })
            .collect();
        if let Some(reads) = t.extra.get("reads").and_then(Value::as_array) {
            fields.push(format!("tables[{}].extra.reads", t.index));
            for r in reads {
                if let Some(l) = r.get("label").and_then(Value::as_str) {
                    parts.push(format!("read \u{201c}{l}\u{201d}"));
                }
            }
        } else if !labelled.is_empty() {
            fields.push(format!("tables[{}].columns[].label", t.index));
            parts.extend(labelled);
        } else if !t.columns.is_empty() {
            fields.push(format!("tables[{}].columns[].name", t.index));
            parts.push(format!(
                "columns {} (no stain labels recorded)",
                t.columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else if !info.traces.is_empty() {
        for t in info.traces.iter().take(8) {
            let w = t
                .extra
                .get("wavelength_nm")
                .and_then(Value::as_f64)
                .map(|w| format!(" at {w:.0} nm"))
                .unwrap_or_default();
            parts.push(format!(
                "trace {} \u{201c}{}\u{201d}{w}",
                t.index,
                t.name.as_deref().unwrap_or("?")
            ));
        }
        fields.push("traces[].name".into());
    }
    Answer {
        topic: "channels".into(),
        answer: if parts.is_empty() {
            "The file records no channels, dyes or detection wavelengths.".into()
        } else {
            format!("{}.", capitalize(&parts.join("; ")))
        },
        fields,
    }
}

fn answer_method(info: &FileInfo, e: &Experiment, gradient: bool) -> Answer {
    let mut fields: Vec<String> = ["method.name", "method.technique"]
        .iter()
        .filter_map(|p| e.origin_of(p).map(|o| o.from.clone()))
        .collect();
    if let Some(m) = &e.method {
        fields.extend(m.parameters.keys().filter_map(|k| {
            e.origin_of(&format!("method.parameters.{k}"))
                .map(|o| o.from.clone())
        }));
    }
    let mut s = method_words(e).map_or_else(
        || "The file records no method name or settings.".to_string(),
        |w| format!("{}.", capitalize(&w)),
    );
    let table = e
        .method
        .as_ref()
        .and_then(|m| m.parameters.get("gradient"))
        .and_then(|q| {
            gradient_words(
                &q.value,
                e.method.as_ref().and_then(|m| m.parameters.get("solvents")),
            )
        });
    let oven = e
        .method
        .as_ref()
        .and_then(|m| m.parameters.get("oven_program"))
        .and_then(|q| oven_words(&q.value));
    if gradient && (table.is_some() || oven.is_some()) {
        for t in table.into_iter().chain(oven) {
            s.push(' ');
            s.push_str(&t);
        }
    } else if gradient {
        let embedded = info
            .spectra
            .first()
            .and_then(|r| r.extra.get("method_summary"))
            .and_then(|m| m.get("embedded_method_bytes"))
            .and_then(Value::as_u64);
        s.push_str(" No gradient table (solvent composition over time) was found in a form OpenReadout reads");
        match embedded {
            Some(n) => {
                let _ = write!(
                    s,
                    "; the file embeds the instrument method ({}): `openreadout info FILE --view full --json` shows each device's method text under `vendor.instrument_method.text`.",
                    human_bytes(n)
                );
            }
            None => s.push_str("; only the method's name and length are recorded here."),
        }
    }
    Answer {
        topic: "method".into(),
        answer: s,
        fields,
    }
}

/// "Oven program: 60 °C for 1 min; then 10 °C/min to 325 °C, held 10 min."
fn oven_words(steps: &Value) -> Option<String> {
    let rows: Vec<String> = steps
        .as_array()?
        .iter()
        .filter_map(|st| {
            let t = st.get("temperature_c")?.as_f64()?;
            let hold = st.get("hold_min").and_then(Value::as_f64).unwrap_or(0.0);
            Some(match st.get("rate_c_per_min").and_then(Value::as_f64) {
                Some(r) => format!(
                    "then {} \u{b0}C/min to {} \u{b0}C, held {} min",
                    num2(r),
                    num2(t),
                    num2(hold)
                ),
                None => format!("{} \u{b0}C for {} min", num2(t), num2(hold)),
            })
        })
        .collect();
    (!rows.is_empty()).then(|| format!("Oven program: {}.", rows.join("; ")))
}

/// "Gradient (7 steps): 0 min 1% B at 500 µL/min; 3 min 15% B …. Solvents: A = …, B = …."
fn gradient_words(steps: &Value, solvents: Option<&Quantity>) -> Option<String> {
    let steps = steps.as_array().filter(|a| !a.is_empty())?;
    let rows: Vec<String> = steps
        .iter()
        .filter_map(|st| {
            let t = st.get("time_min")?.as_f64()?;
            let mix: Vec<String> = st
                .get("percent")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(ch, v)| v.as_f64().map(|p| (ch, p)))
                        .filter(|(_, p)| *p != 0.0 || o.len() == 1)
                        .map(|(ch, p)| format!("{}% {ch}", num2(p)))
                        .collect()
                })
                .unwrap_or_default();
            let flow = st
                .get("flow_ul_min")
                .and_then(Value::as_f64)
                .map(|f| format!(" at {} \u{b5}L/min", num2(f)))
                .unwrap_or_default();
            Some(format!("{} min {}{flow}", num2(t), mix.join(" / ")))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let mut s = format!(
        "Gradient ({}): {}.",
        count(rows.len(), "step"),
        rows.join("; ")
    );
    if let Some(o) = solvents.and_then(|q| q.value.as_object()) {
        let names: Vec<String> = o
            .iter()
            .filter_map(|(ch, n)| n.as_str().map(|n| format!("{ch} = {n}")))
            .collect();
        if !names.is_empty() {
            let _ = write!(s, " Solvents: {}.", names.join(", "));
        }
    }
    Some(s)
}

fn answer_run_length(info: &FileInfo, e: &Experiment) -> Answer {
    let mut v = Vec::new();
    let mut fields = Vec::new();
    if let Some(d) = e.acquisition.as_ref().and_then(|a| a.duration_s) {
        v.push(format!("The recorded data span {}", fmt_duration(d)));
        if let Some(o) = e.origin_of("acquisition.duration_s") {
            fields.push(o.from.clone());
        }
    }
    if let Some(q) = e
        .method
        .as_ref()
        .and_then(|m| m.parameters.get("method_length"))
        && let Some(w) = quantity_words(q)
    {
        v.push(format!("the method is set to run {w}"));
        fields.push("spectra[0].extra.method_summary.method_length_min".into());
    }
    if let Some([a, b]) = info.spectra.first().and_then(|s| s.rt_range_s) {
        v.push(format!(
            "scans run from {} to {} retention time",
            fmt_duration(a),
            fmt_duration(b)
        ));
        fields.push("spectra[0].rt_range_s".into());
    }
    Answer {
        topic: "run_length".into(),
        answer: if v.is_empty() {
            "The file does not record how long the acquisition ran.".into()
        } else {
            format!("{}.", capitalize(&v.join("; ")))
        },
        fields,
    }
}

fn answer_polarity(e: &Experiment) -> Answer {
    let pol = e
        .method
        .as_ref()
        .and_then(|m| m.parameters.get("polarity"))
        .and_then(|q| value_words(&q.value));
    simple(
        "polarity",
        pol.as_deref(),
        |p| {
            if p.contains(',') {
                format!(
                    "Both polarities were acquired ({p}): the instrument switched between positive and negative mode."
                )
            } else {
                format!("The run was acquired in {p} ion mode.")
            }
        },
        "The file does not record the ion polarity.",
        e.origin_of("method.parameters.polarity")
            .map(|o| o.from.clone()),
    )
}

fn answer_ms_levels(info: &FileInfo) -> Answer {
    let Some(r) = info.spectra.first() else {
        return Answer {
            topic: "ms_levels".into(),
            answer: "The file holds no mass spectra.".into(),
            fields: vec![],
        };
    };
    let counts = r.extra.get("ms_level_counts").and_then(Value::as_object);
    let mut parts: Vec<String> = Vec::new();
    for l in &r.ms_levels {
        let n = counts
            .and_then(|c| c.get(&l.to_string()))
            .and_then(Value::as_u64);
        parts.push(match n {
            Some(n) => format!("MS{l}: {}", count_u64(n, "scan")),
            None => format!("MS{l}"),
        });
    }
    let mut s = if parts.is_empty() {
        "The file does not list the MS levels of its scans.".to_string()
    } else {
        format!("MS levels present: {}.", parts.join("; "))
    };
    if r.ms_levels.iter().any(|l| *l > 1) {
        s.push_str(" MS1 and MS/MS scans are interleaved, so do not assume the first scans are MS1: `openreadout spectra FILE --ms-level 2 --nth 1` returns the first MS/MS scan (with its precursor m/z).");
    }
    let mut fields = vec![format!("spectra[{}].ms_levels", r.index)];
    if counts.is_some() {
        fields.push(format!("spectra[{}].extra.ms_level_counts", r.index));
    }
    Answer {
        topic: "ms_levels".into(),
        answer: s,
        fields,
    }
}

fn answer_instrument(e: &Experiment) -> Answer {
    let Some(i) = &e.instrument else {
        return Answer {
            topic: "instrument".into(),
            answer: "The file does not record the instrument.".into(),
            fields: vec![],
        };
    };
    let who: Vec<&str> = [i.vendor.as_deref(), i.model.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let mut s = if who.is_empty() {
        "The instrument's make and model are not recorded".to_string()
    } else {
        format!("Acquired on {}", who.join(" "))
    };
    if let Some(k) = &i.kind {
        let _ = write!(s, " ({}, {})", k.label, k.id);
    }
    if let Some(n) = &i.serial {
        let _ = write!(s, ", serial number {n}");
    }
    if let Some(sw) = &i.software {
        let _ = write!(
            s,
            ", with {sw}{}",
            i.software_version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        );
    }
    s.push('.');
    let fields = [
        "instrument.vendor",
        "instrument.model",
        "instrument.serial",
        "instrument.software",
    ]
    .iter()
    .filter_map(|p| e.origin_of(p).map(|o| o.from.clone()))
    .collect();
    Answer {
        topic: "instrument".into(),
        answer: s,
        fields,
    }
}

fn answer_acquired(e: &Experiment) -> Answer {
    let a = e.acquisition.as_ref();
    let start = a.and_then(|a| a.started_at.as_deref());
    let end = a.and_then(|a| a.ended_at.as_deref());
    let saved = a.and_then(|a| a.saved_at.as_deref());
    let mut fields: Vec<String> = Vec::new();
    for p in [
        "acquisition.started_at",
        "acquisition.ended_at",
        "acquisition.saved_at",
    ] {
        if let Some(o) = e.origin_of(p) {
            fields.push(o.from.clone());
        }
    }
    let answer = match (start, end) {
        (Some(s), Some(t)) => format!(
            "Acquired from {} to {}{}.",
            readable_time(s),
            readable_time(t),
            if has_zone(s) {
                " (UTC or the recorded offset)"
            } else {
                " (local time; no zone recorded)"
            }
        ),
        (Some(s), None) => format!(
            "Acquisition started {}{}.",
            readable_time(s),
            if has_zone(s) {
                " (UTC or the recorded offset)"
            } else {
                " (local time; no zone recorded)"
            }
        ),
        _ => match saved {
            Some(s) => format!(
                "The file does not record when it was acquired; it was saved {}{}, after the measurement.",
                readable_time(s),
                if has_zone(s) {
                    " (UTC or the recorded offset)"
                } else {
                    " (local time; no zone recorded)"
                }
            ),
            None => "The file does not record when it was acquired.".into(),
        },
    };
    Answer {
        topic: "acquired".into(),
        answer,
        fields,
    }
}

fn answer_technique(e: &Experiment) -> Answer {
    let mut v = Vec::new();
    if let Some(t) = e.method.as_ref().and_then(|m| m.technique.as_ref()) {
        v.push(format!("Technique: {} ({})", t.label, t.id));
    }
    for m in e.measurements.iter().take(4) {
        v.push(format!(
            "{} {}: {}",
            m.kind.array(),
            join_indices(&m.indices),
            m.what
        ));
    }
    Answer {
        topic: "technique".into(),
        answer: if v.is_empty() {
            "The reader could not describe what was measured.".into()
        } else {
            format!("{}.", v.join(". "))
        },
        fields: vec![
            "experiment.method.technique".into(),
            "experiment.measurements".into(),
        ],
    }
}

fn join_indices(idx: &[u32]) -> String {
    match idx {
        [one] => one.to_string(),
        _ if idx.len() <= 6 => idx
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        _ => format!("{}–{}", idx[0], idx[idx.len() - 1]),
    }
}

// ---------- formatting helpers ----------

fn article(word: &str) -> &'static str {
    let w = word.trim_start().to_ascii_lowercase();
    let vowel_sound = w.starts_with(['a', 'e', 'i', 'o', 'u'])
        || w.starts_with("fcs")
        || w.starts_with("nd2")
        || w.starts_with("lif")
        || w.starts_with("mrc")
        || w.starts_with('8')
        || w == "11"
        || w == "18"
        || w.starts_with("11 ")
        || w.starts_with("18 ")
        || w.starts_with("11-")
        || w.starts_with("18-");
    let exceptions = w.starts_with("uni") || w.starts_with("one") || w.starts_with("eu");
    if vowel_sound && !exceptions {
        "an"
    } else {
        "a"
    }
}

fn count(n: usize, noun: &str) -> String {
    count_u64(n as u64, noun)
}

fn count_u64(n: u64, noun: &str) -> String {
    let word = if n == 1 {
        noun.to_string()
    } else if let Some(stem) = noun.strip_suffix("trum") {
        format!("{stem}tra")
    } else if noun.ends_with("interest") {
        noun.replacen("region", "regions", 1)
    } else if let Some(stem) = noun.strip_suffix("copy") {
        format!("{stem}copies")
    } else if noun.ends_with('s') || noun.ends_with('x') {
        format!("{noun}es")
    } else {
        format!("{noun}s")
    };
    if n == 1 && !noun.starts_with("region") {
        format!("one {word}")
    } else {
        format!("{} {word}", fmt_count(n))
    }
}

fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Up to three significant digits, no trailing zeros.
fn sig3(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let mag = v.abs().log10().floor() as i32;
    let decimals = (2 - mag).clamp(0, 6) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn trim_num(v: f64) -> String {
    sig3(v)
}

/// At most two decimals, no trailing zeros (`15.51`, `10.2`, `500`).
fn num2(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn fmt_um(v: f64) -> String {
    if v < 0.1 {
        format!("{} nm", sig3(v * 1000.0))
    } else {
        format!("{} µm", sig3(v))
    }
}

fn fmt_len(um: f64) -> String {
    if um >= 1000.0 {
        format!("{} mm", sig3(um / 1000.0))
    } else {
        fmt_um(um)
    }
}

fn fmt_duration(s: f64) -> String {
    let a = s.abs();
    if a == 0.0 {
        "0 s".into()
    } else if a < 1e-3 {
        format!("{} µs", sig3(s * 1e6))
    } else if a < 1.0 {
        format!("{} ms", sig3(s * 1e3))
    } else if a < 120.0 {
        format!("{} s", sig3(s))
    } else if a < 7200.0 {
        format!("{} min", sig3(s / 60.0))
    } else if a < 172_800.0 {
        format!("{} h", sig3(s / 3600.0))
    } else {
        format!("{} days", sig3(s / 86_400.0))
    }
}

fn fmt_rate(hz: f64) -> String {
    if hz >= 1e6 {
        format!("{} MHz", sig3(hz / 1e6))
    } else if hz >= 1e3 {
        format!("{} kHz", sig3(hz / 1e3))
    } else {
        format!("{} Hz", sig3(hz))
    }
}

fn fmt_volume_nl(nl: f64) -> String {
    if nl >= 1000.0 {
        format!("{} µL", sig3(nl / 1000.0))
    } else {
        format!("{} nL", sig3(nl))
    }
}

fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} bytes")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// `2017-06-06T09:15:06.980Z` → `2017-06-06`.
fn date_part(ts: &str) -> Option<&str> {
    (ts.len() >= 10 && ts.as_bytes()[4] == b'-').then(|| &ts[..10])
}

/// `2017-06-06T09:15:06.980Z` → `2017-06-06 09:15:06`.
fn readable_time(ts: &str) -> String {
    if ts.len() >= 19 && ts.is_char_boundary(19) && ts.as_bytes()[10] == b'T' {
        format!("{} {}", &ts[..10], &ts[11..19])
    } else {
        ts.to_string()
    }
}

/// Does an ISO-8601 timestamp carry a zone designator (`Z` or `±hh:mm`)?
fn has_zone(ts: &str) -> bool {
    ts.ends_with('Z')
        || (ts.len() > 19
            && ts.is_char_boundary(ts.len() - 6)
            && ts[ts.len() - 6..].starts_with(['+', '-'])
            && ts.as_bytes()[ts.len() - 3] == b':')
}

/// A reader note as a sentence: capitalized unless it starts with an identifier.
fn sentence(s: &str) -> String {
    let s = s.trim();
    let first = s.split_whitespace().next().unwrap_or_default();
    let s = if first.chars().all(|c| c.is_ascii_lowercase()) {
        capitalize(s)
    } else {
        s.to_string()
    };
    if s.ends_with(['.', '!', '?']) {
        s
    } else {
        format!("{s}.")
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn dedup(v: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    v.retain(|x| seen.insert(x.clone()));
}

/// Quote a path for POSIX shells when it has characters the shell would interpret.
fn quote_path(p: &str) -> String {
    let safe = !p.is_empty()
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c));
    if safe {
        p.to_string()
    } else {
        format!("'{}'", p.replace('\'', r"'\''"))
    }
}

/// `dir/name.nd2` + `.c0.ome.tiff` → `dir/name.c0.ome.tiff`.
fn sibling(path: &str, suffix: &str) -> String {
    let (dir, name) = match path.rfind(['/', '\\']) {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let stem = name.rfind('.').map_or(name, |i| &name[..i]);
    format!("{dir}{stem}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::model::{
        ColumnInfo, FormatDescriptor, InstrumentInfo, MosaicInfo, ObjectiveInfo, PhysicalSize,
        SignalChannelInfo,
    };
    use serde_json::json;

    fn file(id: &str, name: &str, family: &str, confidence: Confidence) -> FileInfo {
        FileInfo {
            path: format!("/data/run 1/sample.{id}"),
            size_bytes: 5 * 1024 * 1024,
            format: FormatDescriptor {
                id: id.into(),
                name: name.into(),
                vendor: String::new(),
                extensions: vec![id.into()],
                family: family.into(),
                can_read: true,
                can_write: false,
                confidence,
                known_gaps: vec!["something".into()],
            },
            format_version: Some("3.0".into()),
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            plane_count: 0,
            notes: vec![],
        }
    }

    #[test]
    fn answers_gradient_and_comment_from_the_method() {
        let mut f = file("raw", "Thermo RAW", "mass-spectrometry", Confidence::High);
        f.spectra.push(SpectraInfo {
            index: 0,
            name: Some("QC1".into()),
            scan_count: 2048,
            ms_levels: vec![1],
            rt_range_s: Some([0.7, 1139.5]),
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Thermo Fisher Scientific".into()),
                model: Some("LTQ Orbitrap Discovery".into()),
                ..Default::default()
            }),
            extra: [
                ("comment", json!("2uL injection, top8")),
                (
                    "method_summary",
                    json!({"method_length_min": 19.0,
                           "gradient": {"device": "LegacyDualPump", "solvents": {"A": "water"},
                                        "steps": [{"time_min": 0.0, "flow_ul_min": 500.0, "percent": {"A": 100.0, "B": 0.0}},
                                                  {"time_min": 13.0, "flow_ul_min": 500.0, "percent": {"A": 0.0, "B": 100.0}}]}}),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        });
        let e = openreadout_core::experiment::derive(&f, &openreadout_core::ProvenanceMap::new());
        let ex = explain_with(&f, Some(&e), Some("what was the gradient? any comment?"));
        let text: Vec<&str> = ex.answers.iter().map(|a| a.answer.as_str()).collect();
        assert!(
            text.iter().any(|a| a.contains(
                "Gradient (2 steps): 0 min 100% A at 500 \u{b5}L/min; 13 min 100% B at 500 \u{b5}L/min. Solvents: A = water."
            )),
            "{text:?}"
        );
        assert!(
            text.iter().any(|a| a.contains("2uL injection, top8")),
            "{text:?}"
        );
    }

    fn confocal_stack() -> ImageInfo {
        let mut im = ImageInfo::new(0, 1024, 1024, PixelType::Uint16);
        im.size_c = 2;
        im.size_z = 21;
        im.physical_size = PhysicalSize::micrometres(Some(0.1), Some(0.1), Some(0.5));
        im.channels = vec![
            ChannelInfo {
                index: 0,
                name: Some("DAPI".into()),
                fluorophore: Some("DAPI".into()),
                excitation_nm: Some(405.0),
                emission_nm: Some(461.0),
                acquisition_mode: Some("Laser Scanning Confocal".into()),
                ..ChannelInfo::default()
            },
            ChannelInfo {
                index: 1,
                name: Some("GFP".into()),
                excitation_nm: Some(488.0),
                emission_range_nm: Some([500.0, 550.0]),
                exposure_ms: Some(2.5),
                ..ChannelInfo::default()
            },
        ];
        im.objective = Some(ObjectiveInfo {
            model: Some("Plan-Apochromat 63x/1.40 Oil".into()),
            nominal_magnification: Some(63.0),
            lens_na: Some(1.4),
            immersion: Some("Oil".into()),
        });
        im.instrument = Some(InstrumentInfo {
            manufacturer: Some("Zeiss".into()),
            model: Some("LSM 980".into()),
            software: Some("ZEN".into()),
            software_version: Some("3.4".into()),
            detector: None,
        });
        im.acquired_at = Some("2023-05-04T12:00:00.000Z".into());
        im.extra.insert("bits_significant".into(), json!(12));
        im.finish()
    }

    #[test]
    fn fluorescence_z_stack() {
        let mut f = file("czi", "Zeiss CZI", "microscopy", Confidence::High);
        f.images = vec![confocal_stack()];
        f.plane_count = 42;
        let e = explain(&f);
        assert!(
            e.summary.starts_with(
                "A Zeiss CZI microscopy file (format version 3.0), 5.0 MiB, holding one image."
            ),
            "{}",
            e.summary
        );
        assert!(
            e.summary.contains("It is a 2-channel fluorescence z-stack (21 slices) of 1024 × 1024 pixels at 0.1 µm per pixel, acquired on 2023-05-04."),
            "{}",
            e.summary
        );
        let all = e.paragraphs.join("\n");
        assert!(all.contains("a field of view of 102 µm × 102 µm"), "{all}");
        assert!(
            all.contains("21 focal planes 0.5 µm apart (a depth of 10 µm)"),
            "{all}"
        );
        assert!(
            all.contains(
                "pixels are 16-bit values (0 to 65535), of which the camera filled 12 bits"
            ),
            "{all}"
        );
        assert!(all.contains("0 \u{201c}DAPI\u{201d} (DAPI, excited at 405 nm, blue light at 461 nm, laser scanning confocal)"), "{all}");
        assert!(all.contains("green light detected at 500–550 nm"), "{all}");
        assert!(all.contains("2.5 ms exposure"), "{all}");
        assert!(all.contains("Imaged through a Plan-Apochromat 63x/1.40 Oil objective (63× magnification, numerical aperture 1.4, oil immersion), on a Zeiss LSM 980 instrument, recorded by ZEN 3.4."), "{all}");
        assert!(
            all.contains("Acquisition started 2023-05-04 12:00:00 (UTC)."),
            "{all}"
        );
        // Path with a space is quoted; a z-stack suggests the middle plane.
        assert!(
            e.suggested_commands
                .contains(&"openreadout check '/data/run 1/sample.czi'".to_string())
        );
        assert!(
            e.suggested_commands
                .iter()
                .any(|c| c.contains("--select z=10 -o '/data/run 1/sample.z10.ome.tiff'")),
            "{:?}",
            e.suggested_commands
        );
        assert!(
            e.suggested_commands
                .iter()
                .any(|c| c.contains("--select c=0"))
        );
        assert!(!e.suggested_commands.iter().any(|c| c.contains("ome-zarr")));
        assert!(e.caveats.is_empty(), "{:?}", e.caveats);
    }

    #[test]
    fn rgb_mosaic_pyramid_multi_image() {
        let mut f = file("czi", "Zeiss CZI", "microscopy", Confidence::Medium);
        let mut a = ImageInfo::new(0, 40_000, 30_000, PixelType::Uint8);
        a.samples_per_pixel = 3;
        a.name = Some("Scene 1".into());
        a.physical_size = PhysicalSize::micrometres(Some(0.22), Some(0.22), None);
        a.channels = vec![ChannelInfo {
            index: 0,
            acquisition_mode: Some("Brightfield".into()),
            ..ChannelInfo::default()
        }];
        a.mosaic = Some(MosaicInfo {
            tile_count: 120,
            tile_width: Some(2048),
            tile_height: Some(2048),
            stitched_on_read: true,
        });
        a.pyramid_levels = 6;
        let a = a.finish();
        let mut b = ImageInfo::new(1, 800, 600, PixelType::Uint8);
        b.samples_per_pixel = 3;
        let b = b.finish();
        f.images = vec![a, b];
        f.notes = vec!["pyramid levels are listed but not read".into()];
        let e = explain(&f);
        assert!(e.summary.contains("holding 2 images"), "{}", e.summary);
        assert!(
            e.summary.contains(
                "The first is a colour (RGB) image of 40000 × 30000 pixels at 0.22 µm per pixel."
            ),
            "{}",
            e.summary
        );
        let all = e.paragraphs.join("\n");
        assert!(all.contains("Image 0 (\u{201c}Scene 1\u{201d}): 40000 × 30000 pixels, a field of view of 8.8 mm × 6.6 mm"), "{all}");
        assert!(
            all.contains("red, green and blue 8-bit values (0 to 255) per pixel"),
            "{all}"
        );
        assert!(all.contains("no fluorescence wavelengths"), "{all}");
        assert!(all.contains("image 0 is a mosaic of 120 tiles of 2048 × 2048 pixels, stitched into one picture when read"), "{all}");
        assert!(
            all.contains("from 800 × 600 to 40000 × 30000 pixels"),
            "{all}"
        );
        assert!(
            all.contains("also stores 5 lower-resolution copies"),
            "{all}"
        );
        assert!(
            e.suggested_commands[0].contains("components")
                && e.suggested_commands[1].contains("--format ome-zarr"),
            "{:?}",
            e.suggested_commands
        );
        assert!(e.suggested_commands.iter().any(|c| c.contains("--image 1")));
        assert!(
            e.caveats
                .iter()
                .any(|c| c == "Pyramid levels are listed but not read."),
            "{:?}",
            e.caveats
        );
        assert!(
            e.caveats
                .iter()
                .any(|c| c.contains("medium confidence (1 known gaps)"))
        );
        assert!(e.caveats.iter().any(|c| c.contains("No pixel size")));
        assert!(e.caveats.iter().any(|c| c.contains("does not record when")));
    }

    #[test]
    fn time_series_local_time() {
        let mut f = file("nd2", "Nikon ND2", "microscopy", Confidence::High);
        let mut im = ImageInfo::new(0, 512, 512, PixelType::Uint16);
        im.size_t = 50;
        im.time_increment_s = Some(1.2);
        im.acquired_at = Some("2020-01-02T03:04:05".into());
        im.extra.insert("event_count".into(), json!(3));
        im.extra.insert("frame_records_total".into(), json!(50));
        f.images = vec![im.finish()];
        let e = explain(&f);
        let all = e.paragraphs.join("\n");
        assert!(
            e.summary.contains("a time series (50 time points)"),
            "{}",
            e.summary
        );
        assert!(all.contains("50 time points every 1.2 s"), "{all}");
        assert!(
            all.contains("taken every 1.2 s, covering about 58.8 s"),
            "{all}"
        );
        assert!(all.contains("(local time)"), "{all}");
        assert!(all.contains("records 3 events"), "{all}");
        assert!(all.contains("a record for each of its 50 frames"), "{all}");
        assert!(e.caveats.iter().any(|c| c.contains("no time zone")));
        assert!(
            e.suggested_commands
                .iter()
                .any(|c| c.contains("--all-frames"))
        );
        assert!(
            !e.suggested_commands
                .iter()
                .any(|c| c.contains("--select c=0"))
        );
    }

    #[test]
    fn flow_cytometry_table() {
        let mut f = file(
            "fcs",
            "Flow Cytometry Standard",
            "flow-cytometry",
            Confidence::High,
        );
        f.format_version = Some("3.1".into());
        let cols = ["FSC-A", "SSC-A", "FITC-A", "PE-A", "Time"];
        let mut t = TableInfo {
            index: 0,
            name: Some("tube1.fcs".into()),
            row_count: 250_000,
            columns: cols
                .iter()
                .enumerate()
                .map(|(i, n)| ColumnInfo {
                    index: i as u32,
                    name: (*n).into(),
                    label: (*n == "FITC-A").then(|| "CD3".into()),
                    dtype: "float32".into(),
                    ..ColumnInfo::default()
                })
                .collect(),
            extra: Default::default(),
        };
        t.extra
            .insert("instrument".into(), json!({"model": "LSRFortessa"}));
        t.extra.insert("software".into(), json!("FACSDiva 8"));
        t.extra
            .insert("acquisition_start".into(), json!("2019-03-01T10:00:00"));
        t.extra
            .insert("acquisition_end".into(), json!("2019-03-01T10:05:00"));
        t.extra.insert(
            "spillover".into(),
            json!({"keyword": "SPILL", "parameters": ["FITC-A", "PE-A"], "matrix": [[1.0, 0.1], [0.02, 1.0]]}),
        );
        f.tables = vec![t];
        let e = explain(&f);
        assert!(
            e.summary.starts_with("A Flow Cytometry Standard flow cytometry file (format version 3.1), 5.0 MiB, holding one data set. It holds 250,000 events × 5 parameters."),
            "{}",
            e.summary
        );
        let all = e.paragraphs.join("\n");
        assert!(
            all.contains("FSC-A, SSC-A, FITC-A (CD3), PE-A, Time"),
            "{all}"
        );
        assert!(
            all.contains(
                "2 scatter (cell size and granularity), 2 fluorescence or other, and a time stamp"
            ),
            "{all}"
        );
        assert!(all.contains("Data set 0: measured on the cytometer LSRFortessa; recorded by FACSDiva 8; acquired from 2019-03-01 10:00:00 to 2019-03-01 10:05:00 (local time)"), "{all}");
        assert!(
            all.contains("A 2 × 2 compensation (spillover) matrix is stored (keyword SPILL)"),
            "{all}"
        );
        assert!(
            e.suggested_commands[0].contains("table") && e.suggested_commands[0].contains("--tidy")
        );
        assert!(e.suggested_commands[1].contains("--format csv --labels"));
        assert!(
            e.suggested_commands
                .iter()
                .any(|c| c.contains("--rows 0-9999"))
        );
        assert!(
            e.caveats
                .iter()
                .any(|c| c.contains("raw detector readings"))
        );
        assert!(e.caveats.iter().any(|c| c.contains("no time zone")));
        assert!(
            !e.suggested_commands
                .iter()
                .any(|c| c.contains("ome-tiff") || c.contains("planes"))
        );
    }

    #[test]
    fn traces_and_spectra() {
        let mut f = file("abf", "Axon Binary", "electrophysiology", Confidence::Low);
        f.traces = vec![TraceInfo {
            index: 0,
            name: None,
            sample_rate_hz: 20_000.0,
            sample_count: 10_000,
            sweep_count: 10,
            channels: vec![
                SignalChannelInfo {
                    index: 0,
                    name: "IN 0".into(),
                    unit: Some("pA".into()),
                    dtype: "int16".into(),
                    scale: 1.0,
                    ..SignalChannelInfo::default()
                },
                SignalChannelInfo {
                    index: 1,
                    name: "IN 1".into(),
                    unit: Some("mV".into()),
                    dtype: "int16".into(),
                    scale: 1.0,
                    ..SignalChannelInfo::default()
                },
            ],
            start_s: None,
            extra: Default::default(),
        }];
        f.spectra = vec![SpectraInfo {
            index: 0,
            name: None,
            scan_count: 3456,
            ms_levels: vec![1, 2],
            rt_range_s: Some([0.5, 1800.0]),
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Thermo".into()),
                model: Some("Q Exactive".into()),
                ..InstrumentInfo::default()
            }),
            extra: Default::default(),
        }];
        let e = explain(&f);
        assert!(
            e.summary
                .contains("holding one recording and one acquisition run."),
            "{}",
            e.summary
        );
        let all = e.paragraphs.join("\n");
        assert!(all.contains("Recording 0: 2 channels (IN 0 in pA, IN 1 in mV) sampled at 20 kHz; 10 sweeps of 10,000 samples (500 ms) each."), "{all}");
        assert!(
            all.contains("Run 0: 3,456 mass spectra at levels MS1 and MS2"),
            "{all}"
        );
        assert!(
            all.contains("over 500 ms to 30 min of retention time, on a Thermo Q Exactive."),
            "{all}"
        );
        assert!(e.caveats.iter().any(|c| c.contains("low confidence")));
    }

    #[test]
    fn helpers() {
        assert_eq!(sig3(1.015_310_241), "1.02");
        assert_eq!(sig3(1034.6), "1035");
        assert_eq!(fmt_len(1034.6), "1.03 mm");
        assert_eq!(fmt_duration(3600.0 * 5.0), "5 h");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(count(1, "image"), "one image");
        assert_eq!(count(2, "region of interest"), "2 regions of interest");
        assert_eq!(count_u64(3, "mass spectrum"), "3 mass spectra");
        assert_eq!(article("Nikon ND2"), "a");
        assert_eq!(article("8-bit"), "an");
        assert_eq!(article("8"), "an");
        assert_eq!(article("12"), "a");
        assert_eq!(
            sentence("read_table/export return raw"),
            "read_table/export return raw."
        );
        assert_eq!(sentence("frames are lossy"), "Frames are lossy.");
        assert_eq!(quote_path("a/b.nd2"), "a/b.nd2");
        assert_eq!(quote_path("it's.nd2"), r"'it'\''s.nd2'");
        assert_eq!(sibling("d/x.y.nd2", ".c0.ome.tiff"), "d/x.y.c0.ome.tiff");
        assert!(has_zone("2020-01-01T00:00:00+02:00"));
        assert!(!has_zone("2020-01-01T00:00:00"));
        assert!(has_zone("2020-01-01T00:00:00.5Z"));
    }
}
