//! `Dataset` for a SerialEM `.mdoc` file on its own: one table with a row per section, the
//! files the sections name, and the experiment (voltage, pixel spacing, tilt range, dose).

use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, Table, TableInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Map, Value, json};

use super::{FORMAT_ID, Mdoc, MdocReader, read};

/// Numeric columns of the section table: (record key, unit).
const COLUMNS: &[(&str, Option<&str>)] = &[
    ("section", None),
    ("acquisition_order", None),
    ("tilt_angle_deg", Some("°")),
    ("exposure_dose_e_per_a2", Some("e/Å²")),
    ("prior_dose_e_per_a2", Some("e/Å²")),
    ("cumulative_dose_e_per_a2", Some("e/Å²")),
    ("frame_dose_e_per_a2", Some("e/Å²")),
    ("subframes", None),
    ("exposure_time_s", Some("s")),
    ("defocus_um", Some("µm")),
    ("target_defocus_um", Some("µm")),
    ("magnification", None),
    ("pixel_spacing_a", Some("Å")),
    ("piece_x", Some("px")),
    ("piece_y", Some("px")),
    ("stage_x_um", Some("µm")),
    ("stage_y_um", Some("µm")),
    ("stage_z_um", Some("µm")),
    ("rotation_angle_deg", Some("°")),
    ("intensity", None),
    ("mean", None),
];

/// An opened mdoc file.
#[derive(Debug)]
pub struct MdocDataset {
    path: PathBuf,
    fs: Fs,
    mdoc: Mdoc,
    records: Vec<Map<String, Value>>,
    /// Columns that hold a value in some row.
    columns: Vec<(&'static str, Option<&'static str>)>,
}

impl MdocDataset {
    /// Open and parse an mdoc file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let mdoc = read(input.fs(), input.path())?;
        let records = mdoc.records();
        let columns = COLUMNS
            .iter()
            .copied()
            .filter(|(k, _)| {
                records
                    .iter()
                    .any(|r| r.get(*k).is_some_and(Value::is_number))
            })
            .collect();
        Ok(MdocDataset {
            path: input.path().to_path_buf(),
            fs: input.fs().clone(),
            mdoc,
            records,
            columns,
        })
    }

    /// The parsed file.
    pub fn mdoc(&self) -> &Mdoc {
        &self.mdoc
    }

    /// The files the mdoc names (the image file, each section's frames, gain references) and
    /// whether each is next to it (or in a `frames` folder there).
    fn named_files(&self) -> Vec<Value> {
        let dir = self.path.parent().unwrap_or(Path::new(""));
        let mut names: Vec<(String, &str)> = Vec::new();
        if let Some(f) = self.mdoc.global("ImageFile") {
            names.push((super::file_name(f), "image"));
        }
        for r in &self.records {
            for (k, role) in [
                ("subframe_file", "frames"),
                ("gain_reference", "gain reference"),
                ("defect_file", "defects"),
            ] {
                if let Some(n) = r.get(k).and_then(Value::as_str)
                    && !names.iter().any(|(m, _)| m == n)
                {
                    names.push((n.to_string(), role));
                }
            }
        }
        names
            .into_iter()
            .map(|(n, role)| {
                let found = [
                    dir.join(&n),
                    dir.join("frames").join(&n),
                    dir.join("Frames").join(&n),
                ]
                .into_iter()
                .find(|p| self.fs.metadata(p).is_ok());
                json!({"file": n, "role": role, "found": found.map(|p| p.display().to_string())})
            })
            .collect()
    }
}

impl Dataset for MdocDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("mdoc".into(), Value::Object(self.mdoc.summary()));
        let files = self.named_files();
        let found = files.iter().filter(|f| !f["found"].is_null()).count();
        let mut notes = vec![format!(
            "SerialEM metadata: {} {} section(s); the images are in the files it names ({} of {} found next to it)",
            self.mdoc.sections.len(),
            self.mdoc.sections.first().map_or("", |s| s.kind.as_str()),
            found,
            files.len()
        )];
        if self.mdoc.acquisition_order().is_none() && !self.mdoc.sections.is_empty() {
            notes.push("a section has no readable DateTime: the dose is not accumulated".into());
        }
        extra.insert("files".into(), Value::Array(files));
        let table = TableInfo {
            index: 0,
            name: Some("sections".into()),
            row_count: self.records.len() as u64,
            columns: self
                .columns
                .iter()
                .enumerate()
                .map(|(i, (k, u))| ColumnInfo {
                    index: i as u32,
                    name: (*k).to_string(),
                    label: None,
                    dtype: "float64".into(),
                    unit: u.map(str::to_string),
                    range: None,
                    extra: Default::default(),
                })
                .collect(),
            extra,
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: MdocReader.descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: vec![table],
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: 0,
            notes,
        })
    }

    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        let mut f = openreadout_core::series::Facts::new();
        let s = self.mdoc.summary();
        f.instrument_kind("OBI:0000990");
        if let Some(t) = self.mdoc.titles.first() {
            f.set("instrument.software", "SerialEM", "[T = ...] title");
            if let Some(scope) = t.split("Digitized on").nth(1) {
                let name: Vec<&str> = scope
                    .split_whitespace()
                    .take_while(|w| !w.contains('-'))
                    .collect();
                f.set("instrument.model", &name.join(" "), "[T = ...] title");
            }
        }
        if let Some(v) = s.get("voltage_kv").and_then(Value::as_f64) {
            f.number("voltage", v, "kV", "Voltage");
        }
        if let Some(v) = s.get("pixel_spacing_a").and_then(Value::as_f64) {
            f.number("pixel_spacing", v, "Å", "PixelSpacing");
        }
        if let Some(v) = s.get("tilt_axis_deg").and_then(Value::as_f64) {
            f.number("tilt_axis_angle", v, "°", "[T = ...] title");
        }
        if let Some(v) = s.get("total_dose_e_per_a2").and_then(Value::as_f64) {
            f.plain("total_dose_e_per_a2", v, "ExposureDose of every section");
        }
        let start = self
            .records
            .iter()
            .filter_map(|r| r.get("acquired_at").and_then(Value::as_str))
            .min();
        if let Some(t) = start {
            f.set("acquisition.started_at", t, "DateTime");
        }
        let n = self.records.len();
        let what = match s.get("tilt_range_deg").and_then(Value::as_array) {
            Some(r) if r.len() == 2 => format!(
                "tilt series metadata, {n} tilts from {}° to {}°",
                r[0], r[1]
            ),
            _ => format!("SerialEM metadata, {n} section(s)"),
        };
        f.measurement(
            openreadout_core::experiment::MeasurementKind::Table,
            vec![0],
            what,
            Some("FBbi:00000258"),
        );
        Some(f.build())
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "globals": self.mdoc.globals,
            "titles": self.mdoc.titles,
            "sections": self.mdoc.sections.iter().map(|s| json!({
                "kind": s.kind, "index": s.index, "fields": s.fields,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("tables[].columns", Source::PriorArt),
            ("tables[].extra.mdoc", Source::PriorArt),
            ("tables[].extra.files", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .mdoc
            .sections
            .iter()
            .map(|s| LsEntry {
                kind: "section".into(),
                name: format!("[{} = {}]", s.kind, s.index),
                offset: None,
                size: None,
                image: None,
                details: json!({"fields": s.fields.len()}),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _idx: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "images in an mdoc file",
            "an mdoc holds metadata only; open the stack or movie it names (info → tables[0].extra.files)",
        ))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} out of range (an mdoc file has one table)"
            )));
        }
        let start = usize::try_from(first_row)
            .unwrap_or(usize::MAX)
            .min(self.records.len());
        let end = start
            .saturating_add(usize::try_from(max_rows).unwrap_or(usize::MAX))
            .min(self.records.len());
        let columns = self
            .columns
            .iter()
            .map(|(k, _)| {
                self.records[start..end]
                    .iter()
                    .map(|r| r.get(*k).and_then(Value::as_f64).unwrap_or(f64::NAN))
                    .collect()
            })
            .collect();
        Ok(Table {
            table: 0,
            first_row: start as u64,
            columns,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("every line is a `key = value` pair, a `[Kind = N]` section header or a `[T = ...]` title");
        r.performed("sections of one kind are numbered without repeats");
        r.performed("the files the mdoc names are looked up next to it");
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.mdoc.sections {
            if !seen.insert((s.kind.clone(), s.index)) {
                r.push(Finding::warning(
                    "repeated_section",
                    format!("[{} = {}] appears more than once", s.kind, s.index),
                ));
            }
        }
        let missing: Vec<String> = self
            .named_files()
            .iter()
            .filter(|f| f["found"].is_null())
            .filter_map(|f| f["file"].as_str().map(str::to_string))
            .collect();
        if !missing.is_empty() {
            r.push(Finding::info(
                "files_not_found",
                format!(
                    "{} named file(s) are not next to the mdoc (first: {})",
                    missing.len(),
                    missing[0]
                ),
            ));
        }
        Ok(r)
    }
}
