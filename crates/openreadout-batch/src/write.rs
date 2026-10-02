//! Writing tables (CSV, TSV, JSON Lines, JSON, Parquet) and reading them back (for
//! `summarize TABLE`).
//!
//! Files are written under a temporary name next to the destination, read back and checked
//! (row and column counts; Parquet from its footer and record batches), and only then renamed
//! into place. Parquet files carry every column's role, unit and description in the field
//! metadata (`openreadout:role`, `unit`, `description`) and the measure and grain in the file
//! metadata (`openreadout:measure`, `openreadout:grain`), so `batch summarize` knows them again.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{BooleanBuilder, Float64Builder, Int64Builder, StringBuilder};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, Float64Type, Int32Type, Int64Type, UInt32Type, UInt64Type};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use openreadout_core::{Error, Result};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;

use crate::table::{Column, ColumnType, Role, Table, Value, infer, parse_number};

/// A table file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TableFormat {
    /// Comma-separated values with a header row.
    Csv,
    /// Tab-separated values with a header row.
    Tsv,
    /// One JSON object per row and line.
    Jsonl,
    /// `{"columns": [...], "rows": [[...], ...]}`.
    Json,
    /// Apache Parquet (Snappy), with column metadata.
    Parquet,
}

impl TableFormat {
    /// From a file extension (`csv`, `tsv`/`tab`/`txt`, `jsonl`/`ndjson`, `json`, `parquet`/`pq`).
    pub fn from_path(p: &Path) -> Option<TableFormat> {
        let e = p.extension()?.to_str()?.to_ascii_lowercase();
        Some(match e.as_str() {
            "csv" => TableFormat::Csv,
            "tsv" | "tab" | "txt" => TableFormat::Tsv,
            "jsonl" | "ndjson" => TableFormat::Jsonl,
            "json" => TableFormat::Json,
            "parquet" | "pq" => TableFormat::Parquet,
            _ => return None,
        })
    }
    /// Parse a name.
    pub fn parse(s: &str) -> Option<TableFormat> {
        TableFormat::from_path(Path::new(&format!("x.{s}")))
    }
    /// Lower-case name.
    pub fn name(self) -> &'static str {
        match self {
            TableFormat::Csv => "csv",
            TableFormat::Tsv => "tsv",
            TableFormat::Jsonl => "jsonl",
            TableFormat::Json => "json",
            TableFormat::Parquet => "parquet",
        }
    }
}

/// What was written.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct WrittenFile {
    /// The file.
    pub path: String,
    /// Its format.
    pub format: TableFormat,
    /// Data rows.
    pub rows: u64,
    /// Columns.
    pub columns: u64,
    /// Size in bytes.
    pub bytes: u64,
    /// Read back and checked before it was renamed into place.
    pub verified: bool,
}

/// Table-level metadata stored with a Parquet file and in JSON output.
#[derive(Debug, Clone, Default)]
pub struct TableMeta {
    /// Measure id (`stats`, …).
    pub measure: String,
    /// Grain columns.
    pub grain: Vec<String>,
}

fn csv_field(s: &str, sep: char) -> String {
    if s.contains([sep, '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Write `t` as delimited text (header row, then one line per row).
pub fn write_delimited(t: &Table, w: &mut dyn Write, sep: char) -> std::io::Result<()> {
    let head: Vec<String> = t.columns.iter().map(|c| csv_field(&c.name, sep)).collect();
    writeln!(w, "{}", head.join(&sep.to_string()))?;
    for r in &t.rows {
        let cells: Vec<String> = r.iter().map(|v| csv_field(&v.text(), sep)).collect();
        writeln!(w, "{}", cells.join(&sep.to_string()))?;
    }
    Ok(())
}

/// One row as a JSON object.
pub fn row_object(t: &Table, r: &[Value]) -> serde_json::Map<String, serde_json::Value> {
    t.columns
        .iter()
        .zip(r)
        .map(|(c, v)| {
            (
                c.name.clone(),
                serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect()
}

/// Write `t` as JSON Lines (one object per row).
pub fn write_jsonl(t: &Table, w: &mut dyn Write) -> std::io::Result<()> {
    for r in &t.rows {
        let line = serde_json::to_string(&row_object(t, r)).map_err(std::io::Error::other)?;
        writeln!(w, "{line}")?;
    }
    Ok(())
}

fn tmp_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map_or_else(|| "table".into(), |n| n.to_string_lossy().into_owned());
    dest.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

/// Write `t` to `dest` in `format` (from the extension when `None`), verified and atomic.
pub fn save(
    t: &Table,
    dest: &Path,
    format: Option<TableFormat>,
    meta: &TableMeta,
    overwrite: bool,
) -> Result<WrittenFile> {
    let format = format
        .or_else(|| TableFormat::from_path(dest))
        .ok_or_else(|| {
            Error::Usage(format!(
                "cannot tell the table format of {} from its extension; use .csv, .tsv, .jsonl, .json or .parquet",
                dest.display()
            ))
        })?;
    if dest.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            dest.display()
        )));
    }
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let tmp = tmp_path(dest);
    let res = (|| -> Result<()> {
        if format == TableFormat::Parquet {
            write_parquet(t, &tmp, meta)?;
        } else {
            let f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            let mut w = BufWriter::new(f);
            match format {
                TableFormat::Csv => write_delimited(t, &mut w, ','),
                TableFormat::Tsv => write_delimited(t, &mut w, '\t'),
                TableFormat::Jsonl => write_jsonl(t, &mut w),
                TableFormat::Json | TableFormat::Parquet => serde_json::to_writer(
                    &mut w,
                    &serde_json::json!({
                        "measure": meta.measure,
                        "grain": meta.grain,
                        "columns": t.columns,
                        "rows": t.rows,
                    }),
                )
                .map_err(std::io::Error::other),
            }
            .and_then(|()| w.flush())
            .map_err(|e| Error::io(&tmp, e))?;
        }
        // read back
        let (back, _) = read_as(&tmp, format)?;
        if back.rows.len() != t.rows.len()
            || (back.columns.len() != t.columns.len() && !t.rows.is_empty())
        {
            return Err(Error::Other(format!(
                "{}: wrote {} rows × {} columns but read back {} × {}",
                tmp.display(),
                t.rows.len(),
                t.columns.len(),
                back.rows.len(),
                back.columns.len()
            )));
        }
        std::fs::rename(&tmp, dest).map_err(|e| Error::io(dest, e))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res?;
    Ok(WrittenFile {
        path: dest.display().to_string(),
        format,
        rows: t.rows.len() as u64,
        columns: t.columns.len() as u64,
        bytes: std::fs::metadata(dest).map_or(0, |m| m.len()),
        verified: true,
    })
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Id => "id",
        Role::Key => "key",
        Role::Annotation => "annotation",
        Role::Metadata => "metadata",
        Role::Value => "value",
        Role::Error => "error",
    }
}

fn role_of_name(s: &str) -> Option<Role> {
    Some(match s {
        "id" => Role::Id,
        "key" => Role::Key,
        "annotation" => Role::Annotation,
        "metadata" => Role::Metadata,
        "value" => Role::Value,
        "error" => Role::Error,
        _ => return None,
    })
}

fn perr(p: &Path, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("{}: parquet: {e}", p.display()))
}

/// Write `t` as Parquet (Snappy) with column metadata.
pub fn write_parquet(t: &Table, path: &Path, meta: &TableMeta) -> Result<()> {
    let fields: Vec<Field> = t
        .columns
        .iter()
        .map(|c| {
            let dt = match c.kind {
                ColumnType::String => DataType::Utf8,
                ColumnType::Integer => DataType::Int64,
                ColumnType::Float => DataType::Float64,
                ColumnType::Boolean => DataType::Boolean,
            };
            let mut md = HashMap::new();
            md.insert(
                "openreadout:role".to_string(),
                role_name(c.role).to_string(),
            );
            if let Some(u) = &c.unit {
                md.insert("unit".to_string(), u.clone());
            }
            if let Some(d) = &c.description {
                md.insert("description".to_string(), d.clone());
            }
            Field::new(&c.name, dt, true).with_metadata(md)
        })
        .collect();
    let mut smd = HashMap::new();
    smd.insert("openreadout:measure".to_string(), meta.measure.clone());
    smd.insert(
        "openreadout:grain".to_string(),
        serde_json::to_string(&meta.grain).unwrap_or_default(),
    );
    let schema = Arc::new(Schema::new_with_metadata(fields, smd));
    let arrays: Vec<ArrayRef> = t
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| -> ArrayRef {
            let cells = t.rows.iter().map(|r| &r[i]);
            match c.kind {
                ColumnType::String => {
                    let mut b = StringBuilder::new();
                    for v in cells {
                        if v.is_null() {
                            b.append_null();
                        } else {
                            b.append_value(v.text());
                        }
                    }
                    Arc::new(b.finish())
                }
                ColumnType::Integer => {
                    let mut b = Int64Builder::new();
                    for v in cells {
                        match v {
                            Value::Int(x) => b.append_value(*x),
                            _ => b.append_null(),
                        }
                    }
                    Arc::new(b.finish())
                }
                ColumnType::Float => {
                    let mut b = Float64Builder::new();
                    for v in cells {
                        b.append_option(v.as_f64().filter(|_| !matches!(v, Value::Text(_))));
                    }
                    Arc::new(b.finish())
                }
                ColumnType::Boolean => {
                    let mut b = BooleanBuilder::new();
                    for v in cells {
                        match v {
                            Value::Bool(x) => b.append_value(*x),
                            _ => b.append_null(),
                        }
                    }
                    Arc::new(b.finish())
                }
            }
        })
        .collect();
    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(|e| perr(path, e))?;
    let f = File::create(path).map_err(|e| Error::io(path, e))?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_created_by(format!("openreadout {}", env!("CARGO_PKG_VERSION")))
        .build();
    let mut w = ArrowWriter::try_new(f, schema, Some(props)).map_err(|e| perr(path, e))?;
    w.write(&batch).map_err(|e| perr(path, e))?;
    w.close().map_err(|e| perr(path, e))?;
    Ok(())
}

/// Role of a column read from a file without metadata, from its name and type.
pub fn guess_role(name: &str, kind: ColumnType) -> Role {
    match name {
        "path" | "format" | "files" => Role::Id,
        "error" | "error_code" => Role::Error,
        "sample_id" | "sample_well" => Role::Metadata,
        "image" | "image_name" | "well" | "channel" | "channel_name" | "z" | "t" | "trace"
        | "trace_name" | "sweep" | "table" | "table_name" | "parameter" | "label"
        | "population" | "name" | "read" => Role::Key,
        _ if kind == ColumnType::String => Role::Annotation,
        _ => Role::Value,
    }
}

/// Read a table file (format from the extension). Returns the table and its measure id when
/// the file records one.
pub fn read_table(path: &Path) -> Result<(Table, Option<String>)> {
    let format = TableFormat::from_path(path).ok_or_else(|| {
        Error::Usage(format!(
            "{}: not a table file (.csv, .tsv, .jsonl, .json or .parquet)",
            path.display()
        ))
    })?;
    read_as(path, format)
}

fn typed_text_table(names: Vec<String>, rows: Vec<Vec<String>>) -> Table {
    let mut columns = Vec::new();
    let mut out_rows: Vec<Vec<Value>> = vec![Vec::with_capacity(names.len()); rows.len()];
    for (i, n) in names.into_iter().enumerate() {
        let cells: Vec<&str> = rows
            .iter()
            .map(|r| r.get(i).map_or("", String::as_str))
            .collect();
        let numeric = cells.iter().any(|c| !c.trim().is_empty())
            && cells
                .iter()
                .all(|c| c.trim().is_empty() || parse_number(c).is_some());
        let boolean = !numeric
            && cells.iter().any(|c| !c.trim().is_empty())
            && cells
                .iter()
                .all(|c| matches!(c.trim(), "" | "true" | "false"));
        for (r, c) in cells.iter().enumerate() {
            let t = c.trim();
            out_rows[r].push(if t.is_empty() {
                Value::Null
            } else if numeric {
                let x = parse_number(t).unwrap_or(f64::NAN);
                if !t.contains(['.', 'e', 'E']) && x.abs() < 9e15 {
                    Value::Int(x as i64)
                } else {
                    Value::Float(x)
                }
            } else if boolean {
                Value::Bool(t == "true")
            } else {
                Value::Text(t.to_string())
            });
        }
        let kind = infer(out_rows.iter().map(|r| &r[i]));
        columns.push(Column {
            role: guess_role(&n, kind),
            name: n,
            kind,
            unit: None,
            description: None,
        });
    }
    Table {
        columns,
        rows: out_rows,
    }
}

fn read_as(path: &Path, format: TableFormat) -> Result<(Table, Option<String>)> {
    let bad = |m: String| Error::Usage(format!("{}: {m}", path.display()));
    match format {
        TableFormat::Csv | TableFormat::Tsv => {
            let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
            let sep = if format == TableFormat::Tsv {
                '\t'
            } else {
                ','
            };
            let mut recs = crate::sheet::split_records(&text, sep);
            recs.retain(|r| !(r.len() == 1 && r[0].is_empty()));
            if recs.is_empty() {
                return Ok((Table::default(), None));
            }
            let names = recs.remove(0);
            Ok((typed_text_table(names, recs), None))
        }
        TableFormat::Jsonl => {
            let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
            let mut rows = Vec::new();
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let o: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(line).map_err(|e| bad(format!("line {}: {e}", i + 1)))?;
                let mut r = crate::table::Row::new();
                for (k, v) in o {
                    r.set(k, json_value(&v));
                }
                rows.push(r);
            }
            let mut t = crate::table::assemble(rows, &[], &|_| Role::Value);
            for c in &mut t.columns {
                c.role = guess_role(&c.name, c.kind);
            }
            Ok((t, None))
        }
        TableFormat::Json => {
            let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
            // an envelope from `--json`, or the bare {columns, rows}
            let d = v.get("data").unwrap_or(&v);
            let columns: Vec<Column> = serde_json::from_value(
                d.get("columns").cloned().unwrap_or(serde_json::Value::Null),
            )
            .map_err(|e| bad(format!("no `columns` list: {e}")))?;
            let rows: Vec<Vec<Value>> =
                serde_json::from_value(d.get("rows").cloned().unwrap_or_default())
                    .map_err(|e| bad(format!("no `rows` list: {e}")))?;
            let measure = d
                .get("measure")
                .and_then(|m| m.as_str())
                .map(str::to_string);
            Ok((Table { columns, rows }, measure))
        }
        TableFormat::Parquet => {
            let f = File::open(path).map_err(|e| Error::io(path, e))?;
            let b = ParquetRecordBatchReaderBuilder::try_new(f).map_err(|e| perr(path, e))?;
            let schema = b.schema().clone();
            let measure = schema.metadata().get("openreadout:measure").cloned();
            let reader = b.build().map_err(|e| perr(path, e))?;
            let mut columns: Vec<Column> = schema
                .fields()
                .iter()
                .map(|f| {
                    let kind = match f.data_type() {
                        DataType::Int8
                        | DataType::Int16
                        | DataType::Int32
                        | DataType::Int64
                        | DataType::UInt8
                        | DataType::UInt16
                        | DataType::UInt32
                        | DataType::UInt64 => ColumnType::Integer,
                        DataType::Float16 | DataType::Float32 | DataType::Float64 => {
                            ColumnType::Float
                        }
                        DataType::Boolean => ColumnType::Boolean,
                        _ => ColumnType::String,
                    };
                    let md = f.metadata();
                    Column {
                        name: f.name().clone(),
                        kind,
                        unit: md.get("unit").cloned(),
                        role: md
                            .get("openreadout:role")
                            .and_then(|r| role_of_name(r))
                            .unwrap_or_else(|| guess_role(f.name(), kind)),
                        description: md.get("description").cloned(),
                    }
                })
                .collect();
            let mut rows: Vec<Vec<Value>> = Vec::new();
            for batch in reader {
                let batch = batch.map_err(|e| perr(path, e))?;
                let n = batch.num_rows();
                let start = rows.len();
                rows.extend((0..n).map(|_| Vec::with_capacity(columns.len())));
                for (ci, arr) in batch.columns().iter().enumerate() {
                    for (r, row) in rows[start..].iter_mut().enumerate() {
                        row.push(arrow_cell(arr.as_ref(), r));
                    }
                    let _ = ci;
                }
            }
            for c in &mut columns {
                if c.kind == ColumnType::String && c.role == Role::Value {
                    c.role = Role::Annotation;
                }
            }
            Ok((Table { columns, rows }, measure))
        }
    }
}

fn arrow_cell(a: &dyn Array, i: usize) -> Value {
    if a.is_null(i) {
        return Value::Null;
    }
    match a.data_type() {
        DataType::Int64 => Value::Int(a.as_primitive::<Int64Type>().value(i)),
        DataType::Int32 => Value::Int(i64::from(a.as_primitive::<Int32Type>().value(i))),
        DataType::UInt32 => Value::Int(i64::from(a.as_primitive::<UInt32Type>().value(i))),
        DataType::UInt64 => Value::count(a.as_primitive::<UInt64Type>().value(i)),
        DataType::Float64 => Value::float_opt(Some(a.as_primitive::<Float64Type>().value(i))),
        DataType::Float32 => {
            Value::float_opt(Some(f64::from(a.as_primitive::<Float32Type>().value(i))))
        }
        DataType::Boolean => Value::Bool(a.as_boolean().value(i)),
        DataType::Utf8 => Value::Text(a.as_string::<i32>().value(i).to_string()),
        DataType::LargeUtf8 => Value::Text(a.as_string::<i64>().value(i).to_string()),
        _ => Value::Null,
    }
}

fn json_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::float_opt(n.as_f64()), Value::Int),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Row, assemble};

    fn t() -> Table {
        assemble(
            vec![
                Row::new()
                    .with("path", "a, b.fcs")
                    .with("n", 3u32)
                    .with("mean", 1.5)
                    .with("ok", true),
                Row::new()
                    .with("path", "c\"d\".fcs")
                    .with("mean", Value::Null)
                    .with("cond", "x"),
            ],
            &[],
            &|n| if n == "path" { Role::Id } else { Role::Value },
        )
    }

    #[test]
    fn every_format_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let meta = TableMeta {
            measure: "stats".into(),
            grain: vec!["channel".into()],
        };
        for ext in ["csv", "tsv", "jsonl", "json", "parquet"] {
            let p = d.path().join(format!("out.{ext}"));
            let w = save(&t(), &p, None, &meta, false).unwrap();
            assert_eq!(w.rows, 2, "{ext}");
            assert!(save(&t(), &p, None, &meta, false).is_err(), "no overwrite");
            let (back, m) = read_table(&p).unwrap();
            assert_eq!(back.rows.len(), 2, "{ext}");
            assert_eq!(
                back.get(0, "path"),
                &Value::Text("a, b.fcs".into()),
                "{ext}"
            );
            assert_eq!(
                back.get(1, "path"),
                &Value::Text("c\"d\".fcs".into()),
                "{ext}"
            );
            assert_eq!(back.get(0, "mean").as_f64(), Some(1.5), "{ext}");
            assert!(back.get(1, "mean").is_null(), "{ext}");
            if matches!(ext, "json" | "parquet") {
                assert_eq!(m.as_deref(), Some("stats"));
            }
            if ext == "parquet" {
                assert_eq!(back.columns[0].role, Role::Id);
            }
        }
        assert!(save(&t(), &d.path().join("x.xyz"), None, &meta, false).is_err());
    }
}
