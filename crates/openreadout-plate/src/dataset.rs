//! `Dataset` implementation: one long-form table per plate read, the vendor header, a listing,
//! and integrity checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, Table, TableInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::Input;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::grid::{row_label, well_name};
use crate::model::{Block, Export};
use crate::{FORMAT_ID, PlateReader};

/// Columns of every plate table, in order.
pub const TABLE_COLUMNS: [&str; 7] = [
    "well",
    "row",
    "col",
    "read",
    "wavelength_nm",
    "time_s",
    "value",
];

/// Most non-numeric cells listed per table in `info` (all are counted).
const MAX_LISTED: usize = 50;

/// An opened plate-reader export.
#[derive(Debug)]
pub struct PlateDataset {
    path: PathBuf,
    size: u64,
    export: Export,
}

impl PlateDataset {
    /// Open a plate-reader export (text, CSV or spreadsheet). The whole file is parsed here;
    /// it is small (kilobytes to a few megabytes).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let export = crate::load(fs, path)?;
        let size = fs.metadata(path).map_or(0, |m| m.len());
        Ok(PlateDataset {
            path: path.to_path_buf(),
            size,
            export,
        })
    }

    pub(crate) fn export(&self) -> &Export {
        &self.export
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn column(index: u32, name: &str, dtype: &str, unit: Option<&str>, label: &str) -> ColumnInfo {
    ColumnInfo {
        index,
        name: name.into(),
        label: Some(label.into()),
        dtype: dtype.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra: BTreeMap::new(),
    }
}

pub(crate) fn table_info(ex: &Export, b: &Block, index: u32) -> TableInfo {
    let mut well = column(
        0,
        "well",
        "uint32",
        None,
        "well index, row-major from A1 = 0; names in extra.categories",
    );
    well.extra.insert(
        "categories".into(),
        json!(
            (0..b.rows)
                .flat_map(|r| (0..b.cols).map(move |c| well_name(r, c)))
                .collect::<Vec<_>>()
        ),
    );
    well.range = Some([0.0, f64::from((b.rows * b.cols).saturating_sub(1))]);
    let mut row = column(1, "row", "uint16", None, "plate row, 1-based (A = 1)");
    row.range = Some([1.0, f64::from(b.rows)]);
    let mut col = column(2, "col", "uint16", None, "plate column, 1-based");
    col.range = Some([1.0, f64::from(b.cols)]);
    // Name every read in the label when the vendor software calculated some of them, so a
    // `table` reader sees which values were measured without looking up `extra.reads`.
    let read_label = if b.channels.iter().any(|c| c.calculated) && b.channels.len() <= 16 {
        format!(
            "read (measurement channel), 1-based; described in extra.reads: {}",
            b.channels
                .iter()
                .enumerate()
                .map(|(i, c)| c.summary(i))
                .collect::<Vec<_>>()
                .join("; ")
        )
    } else if b.channels.iter().any(|c| c.calculated) {
        let calc: Vec<String> = (0..b.channels.len())
            .filter(|&i| b.channels[i].calculated)
            .map(|i| (i + 1).to_string())
            .collect();
        format!(
            "read (measurement channel), 1-based; described in extra.reads; reads {} were CALCULATED by the vendor software, not measured",
            calc.join(", ")
        )
    } else {
        "read (measurement channel), 1-based; described in extra.reads".to_string()
    };
    let mut read = column(3, "read", "uint16", None, &read_label);
    read.range = Some([1.0, b.channels.len() as f64]);
    let wl = column(
        4,
        "wavelength_nm",
        "float64",
        Some("nm"),
        "wavelength: in a spectral scan the scanned wavelength of the row (excitation, emission or absorbance: extra.reads[].settings.scanned_wavelength), otherwise the detection wavelength (emission, or absorbance wavelength); NaN when none",
    );
    let time = column(
        5,
        "time_s",
        "float64",
        Some("s"),
        "elapsed time of a kinetic read; NaN for endpoint reads",
    );
    let mut value = column(
        6,
        "value",
        "float64",
        None,
        "the value as exported; NaN where the cell was not a number",
    );
    let units: Vec<&str> = b
        .channels
        .iter()
        .filter_map(|c| c.unit.as_deref())
        .collect();
    if !units.is_empty() && units.iter().all(|u| *u == units[0]) {
        value.unit = Some(units[0].to_string());
    }
    let mut extra: BTreeMap<String, Value> = BTreeMap::new();
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            extra.insert(k.into(), v);
        }
    };
    put("export", Some(json!(ex.kind.id())));
    put("manufacturer", ex.kind.manufacturer().map(|m| json!(m)));
    let mut instrument = serde_json::Map::new();
    if let Some(m) = &ex.model {
        instrument.insert("model".into(), json!(m));
    }
    if let Some(s) = &ex.serial {
        instrument.insert("serial_number".into(), json!(s));
    }
    put(
        "instrument",
        (!instrument.is_empty()).then(|| Value::Object(instrument)),
    );
    put("software", ex.kind.software().map(|s| json!(s)));
    put(
        "software_version",
        ex.software_version.as_ref().map(|v| json!(v)),
    );
    put("protocol", ex.protocol.as_ref().map(|v| json!(v)));
    put("experiment", ex.experiment.as_ref().map(|v| json!(v)));
    put("operator", ex.operator.as_ref().map(|v| json!(v)));
    put("plate", Some(json!(b.plate)));
    put("barcode", b.barcode.as_ref().map(|v| json!(v)));
    put("plate_type", b.plate_type.as_ref().map(|v| json!(v)));
    put("plate_rows", Some(json!(b.rows)));
    put("plate_columns", Some(json!(b.cols)));
    put("plate_well_count", Some(json!(b.rows * b.cols)));
    put("declared_well_count", b.declared_wells.map(|v| json!(v)));
    put("wells_measured", Some(json!(b.well_count())));
    put("read_type", b.read_type.map(|t| json!(t.name())));
    let mut modes: Vec<&str> = b
        .channels
        .iter()
        .filter(|c| !c.calculated)
        .map(|c| c.mode.name())
        .collect();
    modes.dedup();
    modes.sort_unstable();
    modes.dedup();
    put("read_modes", Some(json!(modes)));
    put(
        "reads",
        Some(json!(
            b.channels
                .iter()
                .enumerate()
                .map(|(i, c)| c.describe(i))
                .collect::<Vec<_>>()
        )),
    );
    put("temperature_c", b.temperature_c.map(|v| json!(v)));
    let acquired = b.started_at.clone().or_else(|| ex.acquired_at.clone());
    put("acquired_at", acquired.map(|v| json!(v)));
    put("acquired_raw", ex.acquired_raw.as_ref().map(|v| json!(v)));
    put(
        "date_order_assumed",
        ex.date_order_assumed.then(|| json!(true)),
    );
    put("ended_at", b.ended_at.as_ref().map(|v| json!(v)));
    let times = b.time_points();
    put(
        "time_points",
        (!times.is_empty()).then(|| json!(times.len())),
    );
    let bad: Vec<&crate::model::Obs> = b.obs.iter().filter(|o| o.text.is_some()).collect();
    if !bad.is_empty() {
        put("non_numeric_count", Some(json!(bad.len())));
        put(
            "non_numeric",
            Some(json!(
                bad.iter()
                    .take(MAX_LISTED)
                    .map(|o| {
                        let mut m = json!({"well": well_name(o.row, o.col), "read": o.channel + 1, "text": o.text});
                        if let Some(t) = o.time_s {
                            m["time_s"] = json!(t);
                        }
                        m
                    })
                    .collect::<Vec<_>>()
            )),
        );
    }
    for (k, v) in &b.extra {
        extra.insert(k.clone(), v.clone());
    }
    TableInfo {
        index,
        name: Some(b.name.clone()),
        row_count: b.obs.len() as u64,
        columns: vec![well, row, col, read, wl, time, value],
        extra,
    }
}

impl Dataset for PlateDataset {
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        crate::assurance::internal(&self.export.container.describe())
    }

    fn info(&self) -> Result<FileInfo> {
        let ex = &self.export;
        let tables = ex
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| table_info(ex, b, i as u32))
            .collect();
        let mut notes = vec![format!(
            "export dialect `{}`{}; each plate read is one long-form table ({})",
            ex.kind.id(),
            ex.kind
                .software()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default(),
            TABLE_COLUMNS.join(",")
        )];
        notes.extend(ex.notes.iter().cloned());
        if ex
            .blocks
            .iter()
            .flat_map(|b| &b.channels)
            .any(|c| c.calculated)
        {
            notes.push("reads marked `calculated` hold values the vendor software derived (blank correction, ratios, normalizations), not raw reads".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: PlateReader.descriptor(),
            format_version: ex.software_version.clone(),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let ex = &self.export;
        let mut header = serde_json::Map::new();
        for (k, v) in &ex.header {
            let mut key = k.clone();
            let mut n = 2;
            while header.contains_key(&key) {
                key = format!("{k} ({n})");
                n += 1;
            }
            header.insert(key, json!(v));
        }
        Ok(json!({
            "export": ex.kind.id(),
            "container": ex.container.describe(),
            "header": header,
            "sections": ex.sections,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("tables[].columns", Source::Inferred),
            ("tables[].extra.export", Source::Inferred),
            ("tables[].extra.instrument", Source::PriorArt),
            ("tables[].extra.software_version", Source::PriorArt),
            ("tables[].extra.protocol", Source::PriorArt),
            ("tables[].extra.plate", Source::PriorArt),
            ("tables[].extra.barcode", Source::PriorArt),
            ("tables[].extra.plate_type", Source::Inferred),
            ("tables[].extra.plate_well_count", Source::Inferred),
            ("tables[].extra.read_type", Source::PriorArt),
            ("tables[].extra.reads[].mode", Source::PriorArt),
            ("tables[].extra.reads[].wavelength_nm", Source::PriorArt),
            ("tables[].extra.reads[].excitation_nm", Source::PriorArt),
            ("tables[].extra.reads[].emission_nm", Source::PriorArt),
            ("tables[].extra.reads[].calculated", Source::Inferred),
            ("tables[].extra.reads[].origin", Source::Inferred),
            ("tables[].extra.reads[].formula", Source::Inferred),
            ("tables[].extra.temperature_c", Source::Inferred),
            ("tables[].extra.acquired_at", Source::Inferred),
            ("tables[].extra.layout", Source::Inferred),
            ("tables[].extra.non_numeric", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (i, b) in self.export.blocks.iter().enumerate() {
            out.push(LsEntry {
                kind: "table".into(),
                name: b.name.clone(),
                offset: None,
                size: Some(b.obs.len() as u64),
                image: None,
                details: json!({"table": i, "plate": b.plate, "sheet": b.sheet, "line": b.line,
                                "reads": b.channels.len(), "wells": b.well_count(),
                                "geometry": format!("{}x{} ({}{}..{}{})", b.rows, b.cols, row_label(0), 1, row_label(b.rows.saturating_sub(1)), b.cols)}),
            });
            for (ci, c) in b.channels.iter().enumerate() {
                let n = b.obs.iter().filter(|o| o.channel as usize == ci).count();
                out.push(LsEntry {
                    kind: if c.calculated {
                        "calculated-read".into()
                    } else {
                        "read".into()
                    },
                    name: c.label.clone(),
                    offset: None,
                    size: Some(n as u64),
                    image: None,
                    details: {
                        let mut d = c.describe(ci);
                        d["table"] = json!(i);
                        d
                    },
                });
            }
        }
        for (k, v) in &self.export.sections {
            out.push(LsEntry {
                kind: "section".into(),
                name: k.clone(),
                offset: None,
                size: v.as_array().map(|a| a.len() as u64),
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "Plate-reader exports hold well values, not images: use `openreadout export FILE --format csv` (or `--format asm`) or the openreadout_table MCP tool.",
        ))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let b = self.export.blocks.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table index {index} out of range (0..{})",
                self.export.blocks.len()
            ))
        })?;
        let total = b.obs.len() as u64;
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the last row ({total} rows)"
            )));
        }
        let end = first_row.saturating_add(max_rows).min(total);
        let rows = &b.obs[first_row as usize..end as usize];
        let mut cols: Vec<Vec<f64>> = vec![Vec::with_capacity(rows.len()); TABLE_COLUMNS.len()];
        for o in rows {
            let ch = &b.channels[o.channel as usize];
            cols[0].push(f64::from(o.row * b.cols + o.col));
            cols[1].push(f64::from(o.row + 1));
            cols[2].push(f64::from(o.col + 1));
            cols[3].push(f64::from(o.channel + 1));
            cols[4].push(
                o.wavelength_nm
                    .or_else(|| ch.detection_nm())
                    .unwrap_or(f64::NAN),
            );
            cols[5].push(o.time_s.unwrap_or(f64::NAN));
            cols[6].push(o.value);
        }
        Ok(Table {
            table: index,
            first_row,
            columns: cols,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let ex = &self.export;
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(format!(
            "exporter recognised: `{}`; header, sections and plate matrices located",
            ex.kind.id()
        ));
        r.performed("every matrix cell parsed as a number or recorded as non-numeric text");
        r.performed("plate geometry: matrix size and well positions against the declared plate type (well count)");
        r.performed(
            "grid completeness per read and time point (partial plates are reported, not errors)",
        );
        r.performed("reads with an unknown detection mode; duplicate values for one well, read, time and wavelength");
        for f in &ex.findings {
            r.push(f.clone());
        }
        if ex.blocks.is_empty() || ex.blocks.iter().all(|b| b.obs.is_empty()) {
            r.push(Finding::error(
                "no_plate_data",
                "no plate values were found in the export",
            ));
        }
        if ex.date_order_assumed {
            r.push(Finding::info(
                "date_order_assumed",
                format!(
                    "date {:?} is ambiguous (day and month both ≤ 12); read month-first",
                    ex.acquired_raw
                        .as_deref()
                        .or(ex.saved_raw.as_deref())
                        .unwrap_or("")
                ),
            ));
        }
        for b in &ex.blocks {
            for f in &b.findings {
                r.push(f.clone());
            }
            check_block(b, &mut r);
        }
        Ok(r)
    }
}

fn check_block(b: &Block, r: &mut CheckReport) {
    if let Some(n) = b.extra.get("decimal_comma_values").and_then(Value::as_u64) {
        r.push(Finding::warning(
            "decimal_comma",
            format!("{}: {n} cells use a decimal comma (`0,02`) in a dot-decimal export; read as decimals", b.name),
        ));
    }
    // non-numeric cells
    let mut texts: BTreeMap<&str, usize> = BTreeMap::new();
    for o in &b.obs {
        if let Some(t) = &o.text {
            *texts.entry(t.as_str()).or_default() += 1;
        }
    }
    if !texts.is_empty() {
        let n: usize = texts.values().sum();
        let list: Vec<String> = texts.iter().map(|(t, c)| format!("{t:?} ×{c}")).collect();
        r.push(Finding::warning(
            "non_numeric_value",
            format!(
                "{}: {n} cells are not numbers ({}); they are NaN in the table",
                b.name,
                list.join(", ")
            ),
        ));
    }
    // geometry against the declared plate type
    if let Some(w) = b.declared_wells {
        match crate::grid::dims_for_wells(w) {
            Some((rr, cc)) => {
                if b.obs.iter().any(|o| o.row >= rr || o.col >= cc) {
                    r.push(Finding::error(
                        "well_count_mismatch",
                        format!("{}: values lie outside a {w}-well plate ({rr}x{cc}) declared by the plate type", b.name),
                    ));
                } else if (b.rows, b.cols) != (rr, cc) {
                    r.push(Finding::warning(
                        "well_count_mismatch",
                        format!(
                            "{}: matrix is {}x{} but the plate type declares {w} wells",
                            b.name, b.rows, b.cols
                        ),
                    ));
                }
            }
            None => r.push(Finding::info(
                "nonstandard_plate",
                format!(
                    "{}: declared well count {w} is not a standard plate format",
                    b.name
                ),
            )),
        }
    }
    // completeness and duplicates per read / time / wavelength
    let plate = (b.rows * b.cols) as usize;
    let mut groups: BTreeMap<(u32, u64, u64), Vec<(u32, u32)>> = BTreeMap::new();
    for o in &b.obs {
        groups
            .entry((
                o.channel,
                o.time_s.unwrap_or(-1.0).to_bits(),
                o.wavelength_nm.unwrap_or(-1.0).to_bits(),
            ))
            .or_default()
            .push((o.row, o.col));
    }
    let mut partial: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    for ((ch, _, _), wells) in &mut groups {
        let n = wells.len();
        wells.sort_unstable();
        wells.dedup();
        if wells.len() != n {
            r.push(Finding::warning(
                "duplicate_value",
                format!(
                    "{}: read {} has {} values for {} wells at one time point/wavelength",
                    b.name,
                    ch + 1,
                    n,
                    wells.len()
                ),
            ));
        }
        if wells.len() < plate {
            let e = partial.entry(*ch).or_insert((usize::MAX, 0));
            e.0 = e.0.min(wells.len());
            e.1 = e.1.max(wells.len());
        }
    }
    for (ch, (lo, hi)) in partial {
        let c = &b.channels[ch as usize];
        r.push(Finding::info(
            "partial_plate",
            format!(
                "{}: read {} ({}) covers {}{} of {plate} wells",
                b.name,
                ch + 1,
                c.label,
                lo,
                if hi == lo {
                    String::new()
                } else {
                    format!("–{hi}")
                }
            ),
        ));
    }
    for (i, c) in b.channels.iter().enumerate() {
        if !c.calculated && c.mode == crate::model::Mode::Unknown {
            r.push(Finding::warning(
                "unknown_read_mode",
                format!(
                    "{}: read {} ({}) has no recognisable detection mode",
                    b.name,
                    i + 1,
                    c.label
                ),
            ));
        }
    }
}
