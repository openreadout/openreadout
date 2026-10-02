//! `info --tidy`: one row of header metadata per data set, with the same columns (and field
//! aliases) as the index's `experiments.parquet` and `search` (`book/src/guides/lab-shares.md`). Reads headers
//! only.

use openreadout_core::Result;
use openreadout_index::record::{ItemKind, Record, Summary};
use openreadout_index::tables::{Cell, EXPERIMENTS, experiment_row};

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Role, Row, Value};

/// Columns of `info --tidy` when no `--fields` are given.
pub const DEFAULT_INFO_FIELDS: &[&str] = &[
    "size_bytes",
    "sample_id",
    "sample_name",
    "sample_well",
    "sample_barcode",
    "sample_position",
    "instrument_model",
    "instrument_serial",
    "method_name",
    "technique_label",
    "operator",
    "started_at",
    "duration_s",
    "image_count",
    "size_x",
    "size_y",
    "size_z",
    "size_c",
    "size_t",
    "pixel_type",
    "physical_size_x_um",
    "physical_size_z_um",
    "channels",
    "objective_magnification",
    "table_count",
    "table_rows",
    "table_columns",
    "trace_count",
    "trace_channels",
    "sample_rate_hz",
    "scan_count",
    "what",
];

/// Header metadata.
#[derive(Debug, Clone, Default)]
pub struct InfoMeasure {
    /// Columns (index field names or aliases; `all` for every column). Empty: the defaults.
    pub fields: Vec<String>,
}

fn value(c: Cell) -> Value {
    match c {
        Cell::Null => Value::Null,
        Cell::Str(s) => Value::text_opt(Some(&s)),
        Cell::U64(v) => Value::count(v),
        Cell::I64(v) => Value::Int(v),
        Cell::F64(v) => Value::float_opt(Some(v)),
        Cell::Bool(b) => Value::Bool(b),
        Cell::Time(_) | Cell::List(_) | Cell::IntList(_) => {
            let t = c.to_text();
            if t.is_empty() {
                Value::Null
            } else {
                Value::Text(t)
            }
        }
    }
}

impl InfoMeasure {
    fn names(&self) -> Result<Vec<&'static str>> {
        if self.fields.is_empty() {
            return Ok(DEFAULT_INFO_FIELDS.to_vec());
        }
        openreadout_index::search::output_fields(&self.fields)
    }
}

impl Measure for InfoMeasure {
    fn id(&self) -> &'static str {
        "info"
    }
    fn grain(&self) -> Vec<String> {
        Vec::new()
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        EXPERIMENTS
            .iter()
            .map(|c| ColumnDoc {
                name: c.name,
                role: if c.name.starts_with("sample_") {
                    Role::Metadata
                } else {
                    Role::Value
                },
                unit: None,
                description: c.description,
            })
            .collect()
    }
    fn headers_only(&self) -> bool {
        true
    }
    fn fingerprint(&self) -> String {
        format!("info:{:?}", self.fields)
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        let names = self.names()?;
        let dir = it.path.is_dir();
        let size = if dir {
            0
        } else {
            std::fs::metadata(it.path).map_or(0, |m| m.len())
        };
        let e = openreadout_core::experiment::of_dataset(&*it.dataset, it.info);
        let rec = Record {
            path: it.path.to_string_lossy().into_owned(),
            kind: if dir {
                ItemKind::Directory
            } else {
                ItemKind::File
            },
            size: if dir { it.info.size_bytes } else { size },
            format: Some(it.info.format.id.clone()),
            format_name: Some(it.info.format.name.clone()),
            family: Some(it.info.format.family.clone()),
            format_vendor: Some(it.info.format.vendor.clone()),
            format_version: it.info.format_version.clone(),
            summary: Summary::from_info(it.info),
            experiment: (!e.is_empty()).then_some(e),
            notes: it.info.notes.iter().take(8).cloned().collect(),
            ..Record::default()
        };
        let cells = experiment_row(&rec, None);
        let mut row = Row::new();
        for n in names {
            if matches!(n, "path" | "format") {
                continue;
            }
            if let Some(i) = EXPERIMENTS.iter().position(|c| c.name == n) {
                let v = value(cells.get(i).cloned().unwrap_or(Cell::Null));
                if !v.is_null() {
                    row.set(n, v);
                }
            }
        }
        Ok(vec![row])
    }
}
