//! `export --to csv`: one table (e.g. an FCS data set) to a CSV file, written to a temporary
//! file, read back and compared value by value, then renamed into place.
//!
//! Formatting and parsing numbers is most of the work, so both run on the rayon threads, in
//! segments of rows ([`Layout`]). The file is the same for any thread count. The values
//! written and the values read back are each hashed per segment, and the segment hashes are
//! hashed in order.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use openreadout_core::model::{FileInfo, TableExportReport, TraceExportReport};
use openreadout_core::{Dataset, Error, Result};
use rayon::prelude::*;
use xxhash_rust::xxh3::Xxh3;

/// How rows are batched: wide tables get fewer rows per batch, so memory depends on the
/// number of values, not on the number of rows.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// Rows read from the reader at once (a multiple of `segment_rows`).
    chunk_rows: u64,
    /// Rows formatted or parsed per task, and the unit of the read-back hash.
    segment_rows: usize,
}

impl Layout {
    fn for_columns(columns: usize) -> Self {
        let per_row = columns.max(1);
        let pow2_rows = |values: usize| (values / per_row).max(1).next_power_of_two() / 2;
        let segment_rows = pow2_rows(1 << 17).clamp(16, 4096);
        let chunk_rows = pow2_rows(1 << 23).clamp(segment_rows, 65_536);
        Layout {
            chunk_rows: chunk_rows as u64,
            segment_rows,
        }
    }
}

/// Format rows `0..rows` on the rayon threads, a few segments at a time. `row(r, text, hash)`
/// appends row `r` with its newline to `text` and feeds its values to `hash`; `write(text,
/// digest)` receives each segment in order.
fn format_segments(
    rows: usize,
    segment_rows: usize,
    row: impl Fn(usize, &mut String, &mut Xxh3) + Sync,
    mut write: impl FnMut(&str, u128) -> Result<()>,
) -> Result<()> {
    let segments = rows.div_ceil(segment_rows);
    let batch = rayon::current_num_threads().max(1) * 2;
    for start in (0..segments).step_by(batch) {
        let done: Vec<(String, u128)> = (start..(start + batch).min(segments))
            .into_par_iter()
            .map(|seg| {
                let mut text = String::new();
                let mut hash = Xxh3::new();
                for r in seg * segment_rows..((seg + 1) * segment_rows).min(rows) {
                    row(r, &mut text, &mut hash);
                }
                (text, hash.digest128())
            })
            .collect();
        for (text, digest) in done {
            write(&text, digest)?;
        }
    }
    Ok(())
}

/// Read back the rows of `path` after `skip` header lines, parsing on the rayon threads.
/// `parse(row, line, hash)` checks one line (without its newline) and feeds its values to
/// `hash`. Returns the number of rows and the hash of the segment hashes.
fn read_back(
    path: &Path,
    skip: u32,
    segment_rows: usize,
    parse: impl Fn(u64, &str, &mut Xxh3) -> Result<()> + Sync,
) -> Result<(u64, u128)> {
    let io = |e| Error::io(path, e);
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path).map_err(io)?);
    let mut line = Vec::new();
    for _ in 0..skip {
        line.clear();
        reader.read_until(b'\n', &mut line).map_err(io)?;
    }
    let batch = rayon::current_num_threads().max(1) * 2;
    let mut total = Xxh3::new();
    let mut count = 0u64;
    let mut eof = false;
    while !eof {
        let mut segments: Vec<Vec<u8>> = Vec::with_capacity(batch);
        while segments.len() < batch && !eof {
            let mut seg = Vec::new();
            let mut lines = 0;
            while lines < segment_rows {
                if reader.read_until(b'\n', &mut seg).map_err(io)? == 0 {
                    eof = true;
                    break;
                }
                lines += 1;
            }
            if lines > 0 {
                segments.push(seg);
            }
        }
        let base = count;
        let results: Vec<Result<(u64, u128)>> = segments
            .par_iter()
            .enumerate()
            .map(|(i, seg)| {
                let text = std::str::from_utf8(seg)
                    .map_err(|_| Error::Other("read-back: the CSV is not valid UTF-8".into()))?;
                let mut hash = Xxh3::new();
                let mut n = 0u64;
                let first = base + (i * segment_rows) as u64;
                for line in text.split_terminator('\n') {
                    let line = line.strip_suffix('\r').unwrap_or(line);
                    parse(first + n, line, &mut hash)?;
                    n += 1;
                }
                Ok((n, hash.digest128()))
            })
            .collect();
        for r in results {
            let (n, digest) = r?;
            count += n;
            total.update(&digest.to_le_bytes());
        }
    }
    Ok((count, total.digest128()))
}

/// What to export.
#[derive(Debug, Clone, Default)]
pub struct CsvOptions {
    pub table: Option<u32>,
    /// Row range as given on the command line (`A-B`, `A-`, `A`; zero-based, inclusive).
    pub rows: Option<String>,
    pub labels: bool,
    pub overwrite: bool,
    /// Trace files: which sweep (default 0).
    pub sweep: Option<u32>,
    /// Trace files: which trace (default 0).
    pub trace: Option<u32>,
}

/// Parse `A-B` (inclusive), `A-` (to the end) or `A` (one row) into `(first, last_inclusive)`.
pub fn parse_rows(spec: &str) -> Result<(u64, Option<u64>)> {
    let bad = || {
        Error::Usage(format!(
            "--rows {spec:?}: expected A-B, A- or A (zero-based, inclusive)"
        ))
    };
    let s = spec.trim();
    let Some((a, b)) = s.split_once('-') else {
        let a: u64 = s.parse().map_err(|_| bad())?;
        return Ok((a, Some(a)));
    };
    let a: u64 = a.trim().parse().map_err(|_| bad())?;
    let b = b.trim();
    if b.is_empty() {
        return Ok((a, None));
    }
    let b: u64 = b.parse().map_err(|_| bad())?;
    if b < a {
        return Err(bad());
    }
    Ok((a, Some(b)))
}

fn quote(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn canonical_bits(v: f64) -> u64 {
    if v.is_nan() {
        f64::NAN.to_bits()
    } else {
        v.to_bits()
    }
}

#[cfg(test)]
fn format_value(v: f64, single: bool) -> String {
    let mut s = String::new();
    push_value(&mut s, v, single);
    s
}

/// Append the CSV text of `v`: the shortest text that reads back as the same float64 (or, for
/// a float32 column, the same float32).
fn push_value(out: &mut String, v: f64, single: bool) {
    let _ = if single {
        write!(out, "{}", v as f32)
    } else {
        write!(out, "{v}")
    };
}

fn parse_value(s: &str, single: bool) -> Option<f64> {
    if single {
        s.parse::<f32>().ok().map(f64::from)
    } else {
        s.parse::<f64>().ok()
    }
}

fn temp_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(|| "export.csv".into(), |n| n.to_string_lossy().to_string());
    output.with_file_name(format!(".{name}.partial-{}", std::process::id()))
}

pub fn export_csv(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &CsvOptions,
) -> Result<TableExportReport> {
    let info = ds.info()?;
    if opts.sweep.is_some() || opts.trace.is_some() {
        return Err(Error::Usage(
            "--sweep/--trace apply to trace files; this file's tables are selected with --table"
                .into(),
        ));
    }
    if info.tables.is_empty() {
        return Err(Error::unsupported(
            "csv",
            format!("CSV export of a {} file", info.format.name),
            "This file holds images, not tables; use `--to ome-tiff`.",
        ));
    }
    let table_index = opts.table.unwrap_or(0);
    let table = info
        .tables
        .iter()
        .find(|x| x.index == table_index)
        .ok_or_else(|| {
            Error::Usage(format!(
                "--table {table_index} out of range (file has {} tables)",
                info.tables.len()
            ))
        })?
        .clone();
    let total = table.row_count;
    let (first, last) = match &opts.rows {
        Some(s) => parse_rows(s)?,
        None => (0, None),
    };
    if first > total || (first == total && total > 0) {
        return Err(Error::Usage(format!(
            "--rows starts at {first} but table {table_index} has {total} rows"
        )));
    }
    let end = last.map_or(total, |l| l.saturating_add(1).min(total));
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let single: Vec<bool> = table.columns.iter().map(|c| c.dtype == "float32").collect();
    // Categorical columns (plate `well`): the value indexes `extra.categories`; the CSV holds the name.
    let cats: Vec<Option<Vec<String>>> = table
        .columns
        .iter()
        .map(|c| {
            c.extra
                .get("categories")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|x| x.as_str().unwrap_or_default().to_string())
                        .collect()
                })
        })
        .collect();
    let lookup: Vec<Option<std::collections::HashMap<&str, f64>>> = cats
        .iter()
        .map(|c| {
            c.as_ref().map(|names| {
                names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (n.as_str(), i as f64))
                    .collect()
            })
        })
        .collect();
    let ncols = table.columns.len();
    let layout = Layout::for_columns(ncols);
    let tmp = temp_path(output);
    let result = (|| -> Result<(u64, u64)> {
        let file = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut w = BufWriter::new(file);
        let io = |e| Error::io(&tmp, e);
        let names: Vec<String> = table.columns.iter().map(|c| quote(&c.name)).collect();
        writeln!(w, "{}", names.join(",")).map_err(io)?;
        if opts.labels {
            let labels: Vec<String> = table
                .columns
                .iter()
                .map(|c| quote(c.label.as_deref().unwrap_or("")))
                .collect();
            writeln!(w, "{}", labels.join(",")).map_err(io)?;
        }
        let mut hash = Xxh3::new();
        let mut pos = first;
        while pos < end {
            let n = layout.chunk_rows.min(end - pos);
            let chunk = ds.read_table(table_index, pos, n)?;
            let rows = chunk.columns.first().map_or(0, Vec::len);
            if chunk.columns.len() != ncols || rows as u64 != n {
                return Err(Error::Other(format!(
                    "reader returned {rows} rows × {} columns for a request of {n} × {ncols}",
                    chunk.columns.len()
                )));
            }
            format_segments(
                rows,
                layout.segment_rows,
                |r, line, hash| {
                    for (c, col) in chunk.columns.iter().enumerate() {
                        if c > 0 {
                            line.push(',');
                        }
                        let v = col[r];
                        match cats[c].as_ref().and_then(|n| {
                            (v >= 0.0 && v.fract() == 0.0)
                                .then(|| n.get(v as usize))
                                .flatten()
                        }) {
                            Some(name) => line.push_str(&quote(name)),
                            None => push_value(line, v, single[c]),
                        }
                        hash.update(&canonical_bits(v).to_le_bytes());
                    }
                    line.push('\n');
                },
                |text, digest| {
                    hash.update(&digest.to_le_bytes());
                    w.write_all(text.as_bytes()).map_err(io)
                },
            )?;
            pos += n;
        }
        w.flush().map_err(io)?;
        drop(w);
        let skip = 1 + u32::from(opts.labels);
        let (count, back) = read_back(&tmp, skip, layout.segment_rows, |row, text, back| {
            let mut fields = 0;
            for (c, s) in text.split(',').enumerate() {
                fields += 1;
                if c >= ncols {
                    continue;
                }
                let named = lookup[c]
                    .as_ref()
                    .and_then(|m| m.get(s.trim_matches('"')).copied());
                let v = named.or_else(|| parse_value(s, single[c])).ok_or_else(|| {
                    Error::Other(format!("read-back row {row}: {s:?} is not a number"))
                })?;
                back.update(&canonical_bits(v).to_le_bytes());
            }
            if fields != ncols {
                return Err(Error::Other(format!(
                    "read-back row {row} has {fields} fields, expected {ncols}"
                )));
            }
            Ok(())
        })?;
        if count != end - first || back != hash.digest128() {
            return Err(Error::Other(
                "CSV read-back did not match the source values".into(),
            ));
        }
        Ok((
            count,
            std::fs::metadata(&tmp)
                .map_err(|e| Error::io(&tmp, e))?
                .len(),
        ))
    })();
    let (rows_written, bytes_written) = match result {
        Ok(v) => v,
        Err(e) => {
            if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
                std::fs::remove_file(&tmp).ok();
            }
            return Err(e);
        }
    };
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    Ok(TableExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "csv".into(),
        table: table_index,
        first_row: first,
        rows_written,
        columns_written: ncols as u32,
        header_lines: 1 + u32::from(opts.labels),
        bytes_written,
        verified: true,
    })
}

/// A trace's regular abscissa from `extra.axis` (`first`, `step`) when it is not time:
/// `(first, step, column name)`, the name being `<quantity>_<unit>` (`chemical_shift_ppm`).
fn regular_axis(t: &openreadout_core::model::TraceInfo) -> Option<(f64, f64, String)> {
    let a = t.extra.get("axis")?;
    let quantity = a.get("quantity").and_then(|q| q.as_str()).unwrap_or("x");
    if quantity == "time" {
        return None;
    }
    let first = a.get("first")?.as_f64()?;
    let step = a.get("step")?.as_f64()?;
    let name = match a.get("unit").and_then(|u| u.as_str()) {
        Some(u) if !u.is_empty() => format!("{quantity}_{u}"),
        _ => quantity.to_string(),
    };
    Some((first, step, name))
}

/// Header of a trace channel column: `name (unit)`.
fn trace_column(name: &str, unit: Option<&str>) -> String {
    match unit {
        Some(u) if !u.is_empty() => quote(&format!("{name} ({u})")),
        _ => quote(name),
    }
}

/// `export --to csv` of one sweep of a trace: `time_s` (seconds on the clock `trace` reports:
/// the trace's `start_s` + sample index / rate for a single sweep, from the sweep start for
/// multi-sweep traces) and one column per channel in physical units; written to a temporary file,
/// read back and compared, then renamed into place.
pub fn export_trace_csv(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    input: &Path,
    output: &Path,
    opts: &CsvOptions,
) -> Result<TraceExportReport> {
    if opts.table.is_some() {
        return Err(Error::Usage(
            "--table applies to tabular files; this file holds traces (use --trace/--sweep)".into(),
        ));
    }
    let trace_index = opts.trace.unwrap_or(0);
    let sweep = opts.sweep.unwrap_or(0);
    let t = info
        .traces
        .iter()
        .find(|x| x.index == trace_index)
        .ok_or_else(|| {
            Error::Usage(format!(
                "--trace {trace_index} out of range (file has {} traces)",
                info.traces.len()
            ))
        })?
        .clone();
    if sweep >= t.sweep_count {
        return Err(Error::Usage(format!(
            "--sweep {sweep} out of range (trace {trace_index} has {} sweeps)",
            t.sweep_count
        )));
    }
    let total = openreadout_core::trace::sweep_samples(&t, sweep);
    let (first, last) = match &opts.rows {
        Some(s) => parse_rows(s)?,
        None => (0, None),
    };
    if first > total || (first == total && total > 0) {
        return Err(Error::Usage(format!(
            "--rows starts at {first} but sweep {sweep} has {total} samples"
        )));
    }
    let end = last.map_or(total, |l| l.saturating_add(1).min(total));
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let rate = t.sample_rate_hz;
    // A regular abscissa that is not time (the ppm axis of an NMR spectrum, the wavenumber axis
    // of an IR spectrum, ...) replaces the time column.
    let axis = regular_axis(&t);
    let origin = openreadout_core::trace::sweep_origin_s(&t);
    let time = |i: u64| match &axis {
        Some((first, step, _)) => first + step * i as f64,
        None if rate > 0.0 => origin + i as f64 / rate,
        None => i as f64,
    };
    let nch = t.channels.len();
    let layout = Layout::for_columns(nch + 1);
    let tmp = temp_path(output);
    let result = (|| -> Result<(u64, u64)> {
        let file = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut w = BufWriter::new(file);
        let io = |e| Error::io(&tmp, e);
        let mut names = vec![match &axis {
            Some((_, _, name)) => quote(name),
            None if rate > 0.0 => "time_s".to_string(),
            None => "sample".to_string(),
        }];
        names.extend(
            t.channels
                .iter()
                .map(|c| trace_column(&c.name, c.unit.as_deref())),
        );
        writeln!(w, "{}", names.join(",")).map_err(io)?;
        let mut hash = Xxh3::new();
        let mut pos = first;
        while pos < end {
            let n = layout.chunk_rows.min(end - pos);
            let chunk = ds.read_trace(trace_index, sweep, pos, n)?;
            let rows = chunk.channels.first().map_or(0, Vec::len);
            if chunk.channels.len() != nch || rows as u64 != n {
                return Err(Error::Other(format!(
                    "reader returned {rows} samples x {} channels for a request of {n} x {nch}",
                    chunk.channels.len()
                )));
            }
            format_segments(
                rows,
                layout.segment_rows,
                |r, line, hash| {
                    let tv = time(pos + r as u64);
                    push_value(line, tv, false);
                    hash.update(&canonical_bits(tv).to_le_bytes());
                    for col in &chunk.channels {
                        line.push(',');
                        push_value(line, col[r], false);
                        hash.update(&canonical_bits(col[r]).to_le_bytes());
                    }
                    line.push('\n');
                },
                |text, digest| {
                    hash.update(&digest.to_le_bytes());
                    w.write_all(text.as_bytes()).map_err(io)
                },
            )?;
            pos += n;
        }
        w.flush().map_err(io)?;
        drop(w);
        let (count, back) = read_back(&tmp, 1, layout.segment_rows, |row, text, back| {
            let mut fields = 0;
            for s in text.split(',') {
                fields += 1;
                let v = parse_value(s, false).ok_or_else(|| {
                    Error::Other(format!("read-back row {row}: {s:?} is not a number"))
                })?;
                back.update(&canonical_bits(v).to_le_bytes());
            }
            if fields != nch + 1 {
                return Err(Error::Other(format!(
                    "read-back row {row} has {fields} fields, expected {}",
                    nch + 1
                )));
            }
            Ok(())
        })?;
        if count != end - first || back != hash.digest128() {
            return Err(Error::Other(
                "CSV read-back did not match the source values".into(),
            ));
        }
        Ok((
            count,
            std::fs::metadata(&tmp)
                .map_err(|e| Error::io(&tmp, e))?
                .len(),
        ))
    })();
    let (samples_written, bytes_written) = match result {
        Ok(v) => v,
        Err(e) => {
            if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
                std::fs::remove_file(&tmp).ok();
            }
            return Err(e);
        }
    };
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    Ok(TraceExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "csv".into(),
        trace: trace_index,
        sweep,
        first_sample: first,
        samples_written,
        channels_written: nch as u32,
        bytes_written,
        verified: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_syntax() {
        assert_eq!(parse_rows("0-99").unwrap(), (0, Some(99)));
        assert_eq!(parse_rows("10-").unwrap(), (10, None));
        assert_eq!(parse_rows("7").unwrap(), (7, Some(7)));
        assert!(parse_rows("5-2").is_err());
        assert!(parse_rows("a-b").is_err());
    }

    #[test]
    fn quoting_and_floats() {
        assert_eq!(quote("FSC-A"), "FSC-A");
        assert_eq!(quote("CD45, FITC"), "\"CD45, FITC\"");
        assert_eq!(quote("a\"b"), "\"a\"\"b\"");
        let v = f64::from(0.1f32);
        let s = format_value(v, true);
        assert_eq!(s, "0.1");
        assert_eq!(parse_value(&s, true), Some(v));
        assert_eq!(format_value(1023.0, false), "1023");
        assert_eq!(trace_column("IN 0", Some("pA")), "IN 0 (pA)");
        assert_eq!(trace_column("a,b", None), "\"a,b\"");
    }

    #[test]
    fn layout_keeps_segments_aligned_and_batches_small() {
        for columns in [0, 1, 2, 48, 385, 4000, 1 << 20] {
            let l = Layout::for_columns(columns);
            assert!(l.segment_rows.is_power_of_two() && (16..=4096).contains(&l.segment_rows));
            assert_eq!(l.chunk_rows % l.segment_rows as u64, 0, "{columns} columns");
            assert!(l.chunk_rows <= 65_536);
            if columns <= 1 << 12 {
                // At most 8 Mi values per chunk.
                assert!(l.chunk_rows * columns.max(1) as u64 <= 1 << 23);
            }
        }
    }
}
