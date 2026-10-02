//! Flat result rows (peak rows, compound rows, …) as CSV, TSV or JSON Lines text. Columns are
//! the union of the rows' keys in first-seen order; nested values are written as JSON.

use serde::Serialize;

use openreadout_core::{Error, Result};

/// Text layout of a row file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowFormat {
    /// Comma-separated, RFC 4180 quoting.
    Csv,
    /// Tab-separated.
    Tsv,
    /// One JSON object per line.
    Jsonl,
}

impl RowFormat {
    /// From a file extension (`csv`, `tsv`/`txt`, `jsonl`/`ndjson`).
    pub fn from_path(path: &std::path::Path) -> Option<RowFormat> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "csv" => Some(RowFormat::Csv),
            "tsv" | "txt" | "tab" => Some(RowFormat::Tsv),
            "jsonl" | "ndjson" => Some(RowFormat::Jsonl),
            _ => None,
        }
    }
}

fn cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn quote(s: &str, delim: char) -> String {
    if s.contains(delim) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Render rows in `format`.
pub fn render<T: Serialize>(rows: &[T], format: RowFormat) -> Result<String> {
    let values: Vec<serde_json::Map<String, serde_json::Value>> = rows
        .iter()
        .map(|r| match serde_json::to_value(r) {
            Ok(serde_json::Value::Object(m)) => Ok(m),
            Ok(_) => Err(Error::Other("row is not an object".into())),
            Err(e) => Err(Error::Other(format!("row: {e}"))),
        })
        .collect::<Result<_>>()?;
    if format == RowFormat::Jsonl {
        let mut out = String::new();
        for v in values {
            out.push_str(
                &serde_json::to_string(&v).map_err(|e| Error::Other(format!("row: {e}")))?,
            );
            out.push('\n');
        }
        return Ok(out);
    }
    let delim = if format == RowFormat::Tsv { '\t' } else { ',' };
    let mut cols: Vec<String> = Vec::new();
    for v in &values {
        for k in v.keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    let mut out = cols
        .iter()
        .map(|c| quote(c, delim))
        .collect::<Vec<_>>()
        .join(&delim.to_string());
    out.push('\n');
    for v in &values {
        let line: Vec<String> = cols
            .iter()
            .map(|c| quote(&v.get(c).map(cell).unwrap_or_default(), delim))
            .collect();
        out.push_str(&line.join(&delim.to_string()));
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct R {
        a: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        b: Option<String>,
    }

    #[test]
    fn csv_union_of_columns() {
        let rows = [
            R { a: 1.5, b: None },
            R {
                a: 2.0,
                b: Some("x, \"y\"".into()),
            },
        ];
        let s = render(&rows, RowFormat::Csv).unwrap();
        assert_eq!(s, "a,b\n1.5,\n2.0,\"x, \"\"y\"\"\"\n");
        let j = render(&rows, RowFormat::Jsonl).unwrap();
        assert_eq!(j.lines().count(), 2);
        assert_eq!(
            RowFormat::from_path(std::path::Path::new("a.TSV")),
            Some(RowFormat::Tsv)
        );
        assert_eq!(render::<R>(&[], RowFormat::Csv).unwrap(), "\n");
    }
}
