//! The model both readers fill, and the one `Dataset` that serves it: a table of samples (one
//! row per measurement, with units) and, when the export holds spectra, one absorbance trace
//! with one sweep per sample on a wavelength axis.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity, Sample,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::vocab;
use openreadout_core::{ColumnInfo, Error, Result};
use serde_json::{Map, Value, json};

/// One column of the samples table.
#[derive(Debug, Clone)]
pub(crate) enum Column {
    /// Numbers (NaN where the cell is empty or not a number).
    Numbers {
        name: String,
        label: String,
        unit: Option<String>,
        values: Vec<f64>,
    },
    /// Text, returned as category codes.
    Text {
        name: String,
        label: String,
        values: Vec<String>,
    },
}

impl Column {
    pub(crate) fn name(&self) -> &str {
        match self {
            Column::Numbers { name, .. } | Column::Text { name, .. } => name,
        }
    }
}

/// Spectra on one regular wavelength axis, one per sample row.
#[derive(Debug, Clone)]
pub(crate) struct Spectra {
    /// First wavelength, nm.
    pub(crate) first_nm: f64,
    /// Wavelength step, nm.
    pub(crate) step_nm: f64,
    /// The y quantity's label (`absorbance (10 mm)`).
    pub(crate) y_label: String,
    /// One spectrum per sample row, all the same length.
    pub(crate) rows: Vec<Vec<f64>>,
}

/// What an export holds.
#[derive(Debug, Clone, Default)]
pub(crate) struct Parsed {
    /// Our id of the export layout (`nanodrop-eight`, `qubit-flex`).
    pub(crate) layout: &'static str,
    /// The instrument as the layout or the file names it.
    pub(crate) model: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) operator: Option<String>,
    pub(crate) software_version: Option<String>,
    /// NanoDrop application or Qubit assay (`dsDNA`, `Protein A280`, `1x dsDNA High Sensitivity`).
    pub(crate) application: Option<String>,
    pub(crate) columns: Vec<Column>,
    pub(crate) rows: usize,
    pub(crate) spectra: Option<Spectra>,
    /// Header lines and anything else the vendor wrote, names untouched.
    pub(crate) header: Vec<(String, String)>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) notes: Vec<String>,
}

impl Parsed {
    pub(crate) fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name() == name)
    }
    /// The first value of a text column.
    pub(crate) fn first_text(&self, name: &str) -> Option<&str> {
        match self.column(name)? {
            Column::Text { values, .. } => {
                values.iter().map(String::as_str).find(|v| !v.is_empty())
            }
            Column::Numbers { .. } => None,
        }
    }
}

/// An opened export.
#[derive(Debug)]
pub struct SampleTableDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    size: u64,
    p: Parsed,
    vendor: &'static str,
    technique: &'static str,
}

impl SampleTableDataset {
    pub(crate) fn new(
        descriptor: FormatDescriptor,
        path: PathBuf,
        size: u64,
        p: Parsed,
        vendor: &'static str,
        technique: &'static str,
    ) -> Self {
        SampleTableDataset {
            descriptor,
            path,
            size,
            p,
            vendor,
            technique,
        }
    }
}

/// Category codes and labels of a text column, in order of first appearance.
fn categories(values: &[String]) -> (Vec<f64>, Vec<String>) {
    let mut labels: Vec<String> = Vec::new();
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let codes = values
        .iter()
        .map(|v| {
            let n = labels.len();
            let i = *index.entry(v.as_str()).or_insert_with(|| {
                labels.push(v.clone());
                n
            });
            i as f64
        })
        .collect();
    (codes, labels)
}

impl Dataset for SampleTableDataset {
    fn info(&self) -> Result<FileInfo> {
        let p = &self.p;
        let columns = p
            .columns
            .iter()
            .enumerate()
            .map(|(k, c)| match c {
                Column::Numbers {
                    name, label, unit, ..
                } => ColumnInfo {
                    index: k as u32,
                    name: name.clone(),
                    label: Some(label.clone()),
                    dtype: "float64".into(),
                    unit: unit.clone(),
                    range: None,
                    extra: BTreeMap::new(),
                },
                Column::Text {
                    name,
                    label,
                    values,
                } => ColumnInfo {
                    index: k as u32,
                    name: name.clone(),
                    label: Some(label.clone()),
                    dtype: "uint32".into(),
                    unit: None,
                    range: None,
                    extra: BTreeMap::from([(
                        "categories".to_string(),
                        json!(categories(values).1),
                    )]),
                },
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("kind".to_string(), json!("samples"));
        extra.insert("layout".to_string(), json!(p.layout));
        if p.spectra.is_some() {
            extra.insert("spectra_trace".to_string(), json!(0));
        }
        let traces = p
            .spectra
            .as_ref()
            .map(|s| {
                let n = s.rows.first().map_or(0, Vec::len) as u64;
                let last = s.first_nm + s.step_nm * n.saturating_sub(1) as f64;
                let mut extra = BTreeMap::new();
                extra.insert(
                    "axis".to_string(),
                    json!({"quantity": "wavelength", "unit": "nm", "first": s.first_nm,
                           "last": last, "step": s.step_nm, "size": n}),
                );
                extra.insert("data_type".to_string(), json!("UV/VIS SPECTRUM"));
                extra.insert("y_quantity".to_string(), json!("absorbance"));
                extra.insert("spectra_table".to_string(), json!(0));
                vec![TraceInfo {
                    index: 0,
                    name: Some("absorbance".into()),
                    // one sample per step of the wavelength axis
                    sample_rate_hz: if s.step_nm > 0.0 {
                        1.0 / s.step_nm
                    } else {
                        1.0
                    },
                    sample_count: n,
                    sweep_count: s.rows.len() as u32,
                    channels: vec![SignalChannelInfo {
                        index: 0,
                        name: "absorbance".into(),
                        unit: None,
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::from([("label".to_string(), json!(s.y_label))]),
                    }],
                    start_s: None,
                    extra,
                }]
            })
            .unwrap_or_default();
        let mut notes = p.notes.clone();
        if p.spectra.is_some() {
            notes.push(
                "trace 0 holds one absorbance spectrum per sample (sweep k = table row k), on a wavelength axis in nm"
                    .into(),
            );
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: Some(p.layout.to_string()),
            plane_count: 0,
            images: Vec::new(),
            tables: vec![TableInfo {
                index: 0,
                name: Some("samples".into()),
                row_count: p.rows as u64,
                columns,
                extra,
            }],
            spectra: Vec::new(),
            traces,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let m: Map<String, Value> = self
            .p
            .header
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        Ok(
            json!({ "header": Value::Object(m), "columns": self.p.columns.iter().map(|c| match c {
            Column::Numbers { label, .. } | Column::Text { label, .. } => label.clone(),
        }).collect::<Vec<_>>() }),
        )
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("tables".into(), Source::Inferred);
        if self.p.spectra.is_some() {
            p.insert("traces".into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "this export holds a table of samples (table 0), not images".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        self.check_headers()
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("every line parsed; rows counted against the header's columns");
        for f in &self.p.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let Some(s) = self.p.spectra.as_ref().filter(|_| index == 0) else {
            return Err(Error::Usage(format!(
                "trace {index} does not exist ({}): the samples are table 0",
                if self.p.spectra.is_some() {
                    "trace 0 holds the spectra"
                } else {
                    "this export holds no spectra"
                }
            )));
        };
        let row = s.rows.get(sweep as usize).ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} does not exist ({} spectra, one per sample)",
                s.rows.len()
            ))
        })?;
        let n = row.len() as u64;
        let a = first.min(n) as usize;
        let b = first.saturating_add(max).min(n) as usize;
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample: a as u64,
            channels: vec![row[a..b].to_vec()],
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} does not exist (table 0 holds the samples)"
            )));
        }
        let n = self.p.rows as u64;
        let a = first_row.min(n) as usize;
        let b = first_row.saturating_add(max_rows).min(n) as usize;
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: self
                .p
                .columns
                .iter()
                .map(|c| match c {
                    Column::Numbers { values, .. } => values[a..b].to_vec(),
                    Column::Text { values, .. } => categories(values).0[a..b].to_vec(),
                })
                .collect(),
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let p = &self.p;
        let mut exp = Experiment::default();
        let inferred = |from: &str| Origin {
            source: Source::Inferred,
            from: from.into(),
        };
        exp.instrument = Some(ExperimentInstrument {
            vendor: Some(self.vendor.into()),
            model: p.model.clone(),
            serial: p.serial.clone(),
            software: None,
            software_version: p.software_version.clone(),
            // a spectrophotometer; the curated terms have none for a fluorometer
            kind: (self.technique == "CHMO:0000292")
                .then(|| vocab::term("OBI:0400115"))
                .flatten(),
        });
        exp.provenance
            .insert("instrument.vendor".into(), inferred("the export layout"));
        if p.model.is_some() {
            exp.provenance
                .insert("instrument.model".into(), inferred("the export layout"));
        }
        if p.serial.is_some() {
            exp.provenance.insert(
                "instrument.serial".into(),
                inferred("the `Serial Number` header line"),
            );
        }
        if p.software_version.is_some() {
            exp.provenance.insert(
                "instrument.software_version".into(),
                inferred("the software version column"),
            );
        }
        if exp.instrument.as_ref().is_some_and(|i| i.kind.is_some()) {
            exp.provenance
                .insert("instrument.kind".into(), inferred("the instrument family"));
        }
        let mut method = Method {
            name: p.application.clone(),
            technique: vocab::term(self.technique),
            ..Method::default()
        };
        if p.application.is_some() {
            exp.provenance.insert(
                "method.name".into(),
                inferred("the application or assay the export names"),
            );
        }
        exp.provenance
            .insert("method.technique".into(), inferred("the instrument family"));
        if let Some(Column::Numbers { values, .. }) = p.column("dilution_factor")
            && let Some(d) = values.iter().find(|v| v.is_finite())
            && values
                .iter()
                .all(|v| !v.is_finite() || v.to_bits() == d.to_bits())
        {
            method
                .parameters
                .insert("dilution_factor".into(), Quantity::plain(*d));
            exp.provenance.insert(
                "method.parameters.dilution_factor".into(),
                inferred("the `Dilution Factor` column (the same for every sample)"),
            );
        }
        exp.method = Some(method);
        let times: Vec<&str> = match p.column("measured_at") {
            Some(Column::Text { values, .. }) => values
                .iter()
                .map(String::as_str)
                .filter(|v| !v.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        let started = times.iter().min().map(|s| (*s).to_string());
        let ended = times.iter().max().map(|s| (*s).to_string());
        if started.is_some() || p.operator.is_some() {
            exp.acquisition = Some(Acquisition {
                started_at: started.clone(),
                ended_at: ended.filter(|e| Some(e) != started.as_ref()),
                operator: p.operator.clone(),
                ..Acquisition::default()
            });
            if started.is_some() {
                exp.provenance.insert(
                    "acquisition.started_at".into(),
                    inferred("the earliest sample time (local time)"),
                );
            }
            if exp
                .acquisition
                .as_ref()
                .is_some_and(|a| a.ended_at.is_some())
            {
                exp.provenance.insert(
                    "acquisition.ended_at".into(),
                    inferred("the latest sample time (local time)"),
                );
            }
            if p.operator.is_some() {
                exp.provenance.insert(
                    "acquisition.operator".into(),
                    inferred("the user the export names"),
                );
            }
        }
        if p.rows == 1
            && let Some(name) = p.first_text("sample")
        {
            exp.sample = Some(Sample {
                name: Some(name.to_string()),
                ..Sample::default()
            });
            exp.provenance
                .insert("sample.name".into(), inferred("the sample column"));
        }
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Table,
            indices: vec![0],
            what: format!("{} sample measurement(s)", p.rows),
            technique: vocab::term(self.technique),
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        exp.provenance.insert(
            "measurements[0]".into(),
            inferred("the reader's data layout (one row per measurement)"),
        );
        if p.spectra.is_some() {
            exp.measurements.push(Measurement {
                kind: MeasurementKind::Trace,
                indices: vec![0],
                what: "absorbance spectra, one per sample".into(),
                technique: vocab::term("CHMO:0000292"),
                terms: Vec::new(),
                parameters: BTreeMap::new(),
            });
            exp.provenance.insert(
                "measurements[1]".into(),
                inferred("the reader's data layout (one spectrum per sample)"),
            );
        }
        Some(exp)
    }
}

/// Build a text column.
pub(crate) fn text(name: &str, label: &str, values: Vec<String>) -> Column {
    Column::Text {
        name: name.into(),
        label: label.into(),
        values,
    }
}

/// Build a number column.
pub(crate) fn numbers(name: &str, label: &str, unit: Option<&str>, values: Vec<f64>) -> Column {
    Column::Numbers {
        name: name.into(),
        label: label.into(),
        unit: unit.map(str::to_string),
        values,
    }
}

/// Our column name for a header we have no name for: lower case, words joined by `_`.
pub(crate) fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() { "column".into() } else { out }
}
