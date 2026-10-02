//! Extracted chromatograms as a [`Dataset`] with one table, so that the existing CSV, Parquet
//! and Arrow writers (with their read-back verification) export them unchanged.
//!
//! The table is **wide** when every chromatogram has the same retention times (chromatograms
//! computed from the same scans): `rt_min`, then one column per chromatogram named by its
//! label. Otherwise it is **long**: `chromatogram` (0-based index into `chromatograms[]`),
//! `rt_min`, `intensity`.

use std::collections::BTreeMap;

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, FormatDescriptor, LsEntry, Table, TableInfo,
};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::{Confidence, Dataset, Error, Plane, PlaneIndex, Result};

use crate::extract::{Chromatogram, ChromatogramOutput};

/// Chromatograms exposed as one table.
#[derive(Debug, Clone)]
pub struct ChromatogramTable {
    path: String,
    wide: bool,
    columns: Vec<ColumnInfo>,
    values: Vec<Vec<f64>>,
}

fn col(
    index: u32,
    name: &str,
    unit: Option<&str>,
    extra: BTreeMap<String, serde_json::Value>,
) -> ColumnInfo {
    ColumnInfo {
        index,
        name: name.into(),
        label: None,
        dtype: "float64".into(),
        unit: unit.map(str::to_string),
        range: None,
        extra,
    }
}

impl ChromatogramTable {
    /// Build the table (wide or long, see the module docs).
    pub fn new(out: &ChromatogramOutput) -> Self {
        let ch: &[Chromatogram] = &out.chromatograms;
        let wide = !ch.is_empty() && ch.iter().all(|c| c.rt_min == ch[0].rt_min);
        let mut columns = vec![];
        let mut values = vec![];
        if wide {
            columns.push(col(0, "rt_min", Some("min"), BTreeMap::new()));
            values.push(ch[0].rt_min.clone());
            let mut used: Vec<String> = Vec::new();
            for c in ch {
                let mut name = c.label.clone();
                let mut k = 2;
                while used.contains(&name) {
                    name = format!("{} ({k})", c.label);
                    k += 1;
                }
                used.push(name.clone());
                let mut extra = BTreeMap::new();
                extra.insert("kind".into(), serde_json::json!(c.kind));
                extra.insert("source".into(), serde_json::json!(c.source));
                columns.push(col(
                    columns.len() as u32,
                    &name,
                    c.intensity_unit.as_deref(),
                    extra,
                ));
                values.push(c.intensity.clone());
            }
        } else {
            columns.push(col(0, "chromatogram", None, BTreeMap::new()));
            columns.push(col(1, "rt_min", Some("min"), BTreeMap::new()));
            columns.push(col(2, "intensity", None, BTreeMap::new()));
            let mut idx = Vec::new();
            let mut rt = Vec::new();
            let mut y = Vec::new();
            for (k, c) in ch.iter().enumerate() {
                idx.extend(std::iter::repeat_n(k as f64, c.rt_min.len()));
                rt.extend_from_slice(&c.rt_min);
                y.extend_from_slice(&c.intensity);
            }
            values = vec![idx, rt, y];
        }
        Self {
            path: out.path.clone(),
            wide,
            columns,
            values,
        }
    }

    /// True for the wide layout.
    pub fn is_wide(&self) -> bool {
        self.wide
    }
}

impl Dataset for ChromatogramTable {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.clone(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "chromatogram".into(),
                name: "Extracted chromatograms".into(),
                vendor: String::new(),
                extensions: Vec::new(),
                family: "chromatography".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Medium,
                known_gaps: Vec::new(),
            },
            format_version: None,
            images: Vec::new(),
            tables: vec![TableInfo {
                index: 0,
                name: Some(
                    if self.wide {
                        "chromatograms (wide)"
                    } else {
                        "chromatograms (long)"
                    }
                    .into(),
                ),
                row_count: self.values.first().map_or(0, Vec::len) as u64,
                columns: self.columns.clone(),
                extra: BTreeMap::new(),
            }],
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: 0,
            notes: Vec::new(),
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            "chromatogram",
            "image planes",
            "Extracted chromatograms are a table.",
        ))
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new(self.path.clone(), "chromatogram"))
    }
    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} out of range (1 table)"
            )));
        }
        let n = self.values.first().map_or(0, Vec::len) as u64;
        let lo = first_row.min(n) as usize;
        let hi = first_row.saturating_add(max_rows).min(n) as usize;
        Ok(Table {
            table: 0,
            first_row: lo as u64,
            columns: self.values.iter().map(|c| c[lo..hi].to_vec()).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_and_long() {
        let a = Chromatogram {
            label: "TIC".into(),
            rt_min: vec![1.0, 2.0],
            intensity: vec![3.0, 4.0],
            ..Chromatogram::default()
        };
        let mut b = a.clone();
        b.label = "TIC".into();
        let out = ChromatogramOutput {
            chromatograms: vec![a.clone(), b],
            ..ChromatogramOutput::default()
        };
        let mut t = ChromatogramTable::new(&out);
        assert!(t.is_wide());
        let info = t.info().unwrap();
        let names: Vec<&str> = info.tables[0]
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["rt_min", "TIC", "TIC (2)"]);
        assert_eq!(t.read_table(0, 1, 10).unwrap().columns[1], vec![4.0]);
        let mut c = a.clone();
        c.rt_min = vec![1.5];
        c.intensity = vec![9.0];
        let out = ChromatogramOutput {
            chromatograms: vec![a, c],
            ..ChromatogramOutput::default()
        };
        let mut t = ChromatogramTable::new(&out);
        assert!(!t.is_wide());
        let tab = t.read_table(0, 0, 100).unwrap();
        assert_eq!(tab.columns[0], vec![0.0, 0.0, 1.0]);
        assert!(t.read_table(1, 0, 1).is_err());
        assert!(t.read_plane(0, PlaneIndex::default()).is_err());
    }
}
