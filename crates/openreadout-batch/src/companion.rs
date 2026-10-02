//! Files that sit next to instrument data in a lab folder without being data: sample sheets,
//! sequences, plate maps, compound or region lists (CSV, TSV, text, workbooks) and documents
//! (READMEs, PDFs). A batch over the folder skips them and says so, instead of reporting each as
//! an unknown-format error; files that look like data and no reader recognises still are errors.
//!
//! A table is a sheet when its header names what sheets hold (`sample`, `file`, `well`,
//! `compound`, `injection`, `condition`, `group`, `region`, …) and its cells are not almost all
//! numbers (a data export is columns of numbers; a sheet has identifiers). A plate-shaped grid
//! (`sheet::read` finds one) is a plate map.

use std::path::Path;

use crate::sheet::{self, SheetKind};

/// What a companion file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionKind {
    /// A table with one row per sample, file, injection or tube.
    SampleSheet,
    /// Plate-shaped grids (a plate layout).
    PlateMap,
    /// A list of compounds (names with m/z, retention times, concentrations or IDs).
    CompoundList,
    /// A list of integration regions or windows.
    RegionList,
    /// A README, notes, licence or report.
    Document,
}

impl CompanionKind {
    /// Short description for reports.
    pub fn describe(self) -> &'static str {
        match self {
            CompanionKind::SampleSheet => "sample sheet",
            CompanionKind::PlateMap => "plate map",
            CompanionKind::CompoundList => "compound list",
            CompanionKind::RegionList => "region list",
            CompanionKind::Document => "document",
        }
    }
}

/// Largest file inspected (sheets are kilobytes).
const MAX_BYTES: u64 = 16 << 20;
/// Tables with more rows than this are data, whatever their header says.
const MAX_ROWS: usize = 100_000;
/// A table whose cells are at least this fraction numbers is data, not a sheet.
const NUMERIC_DATA: f64 = 0.9;

/// Header words that sheets use (matched against the words of each header cell).
const SHEET_WORDS: &[&str] = &[
    "sample",
    "samples",
    "file",
    "filename",
    "files",
    "path",
    "well",
    "wells",
    "injection",
    "vial",
    "position",
    "condition",
    "treatment",
    "group",
    "genotype",
    "stage",
    "replicate",
    "subject",
    "patient",
    "donor",
    "batch",
    "plate",
    "tube",
    "cell",
    "stimulation",
    "timepoint",
    "time",
    "dose",
    "concentration",
    "conc",
    "role",
    "type",
    "name",
    "id",
    "label",
    "description",
    "sequence",
    "run",
    "strain",
    "animal",
];
const COMPOUND_WORDS: &[&str] = &[
    "compound",
    "compounds",
    "analyte",
    "drug",
    "inhibitor",
    "cas",
    "smiles",
    "formula",
];
const REGION_WORDS: &[&str] = &["region", "regions", "window", "band", "integral"];

fn words(s: &str) -> Vec<String> {
    let lower = s.to_ascii_lowercase();
    let mut out: Vec<String> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    // `sampleid`, `datafile`, `fcsfile`: the known word at either end of a joined token
    let joined: String = lower.chars().filter(char::is_ascii_alphanumeric).collect();
    for w in SHEET_WORDS.iter().chain(COMPOUND_WORDS).chain(REGION_WORDS) {
        if joined != *w && (joined.starts_with(w) || joined.ends_with(w)) && w.len() >= 4 {
            out.push((*w).to_string());
        }
    }
    out
}

fn is_number(s: &str) -> bool {
    let t = s.trim().trim_end_matches('%');
    !t.is_empty() && t.replace(',', "").parse::<f64>().is_ok()
}

fn document(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = name.split('.').next().unwrap_or("");
    if matches!(
        stem,
        "readme" | "license" | "licence" | "copying" | "changelog" | "changes" | "notice"
    ) || stem.starts_with("readme")
    {
        return true;
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "md" | "markdown"
            | "rst"
            | "pdf"
            | "doc"
            | "docx"
            | "odt"
            | "rtf"
            | "html"
            | "htm"
            | "pptx"
    )
}

fn tabular(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "csv" | "tsv" | "tab" | "txt" | "xlsx" | "xlsm" | "xls" | "xlsb" | "ods"
    )
}

/// What `path` is when it is a companion file rather than instrument data; `None` for anything
/// else (including files that could not be read).
pub fn classify(path: &Path) -> Option<CompanionKind> {
    if document(path) {
        return Some(CompanionKind::Document);
    }
    if !tabular(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_BYTES {
        return None;
    }
    let s = sheet::read(path, None).ok()?;
    if s.kind == SheetKind::PlateLayout {
        return Some(CompanionKind::PlateMap);
    }
    if s.rows.is_empty() || s.rows.len() > MAX_ROWS {
        return None;
    }
    let header: Vec<String> = s.columns.iter().flat_map(|c| words(c)).collect();
    let has = |set: &[&str]| header.iter().any(|w| set.contains(&w.as_str()));
    if !(has(SHEET_WORDS) || has(COMPOUND_WORDS) || has(REGION_WORDS)) {
        return None;
    }
    let (mut cells, mut numeric) = (0usize, 0usize);
    for r in &s.rows {
        for c in r.iter().filter(|c| !c.trim().is_empty()) {
            cells += 1;
            numeric += usize::from(is_number(c));
        }
    }
    if cells == 0 || numeric as f64 >= NUMERIC_DATA * cells as f64 {
        return None;
    }
    Some(if has(REGION_WORDS) {
        CompanionKind::RegionList
    } else if has(COMPOUND_WORDS) {
        CompanionKind::CompoundList
    } else {
        CompanionKind::SampleSheet
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn sheets_documents_and_data() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let k = |name: &str, text: &str| classify(&write(dir, name, text));
        assert_eq!(
            k("samples.csv", "data_file,time_h\nRUN-01.D,0\nRUN-02.D,1\n"),
            Some(CompanionKind::SampleSheet)
        );
        assert_eq!(
            k("tubes.csv", "fcs_file,stimulation\nA.fcs,SEB\nB.fcs,none\n"),
            Some(CompanionKind::SampleSheet)
        );
        assert_eq!(
            k(
                "compounds.csv",
                "sample_id,compound\nSPL1,KX-1007\nSPL2,KX-1014\n"
            ),
            Some(CompanionKind::CompoundList)
        );
        assert_eq!(
            k("regions.csv", "region,from_ppm,to_ppm\nA,7.0,7.5\n"),
            Some(CompanionKind::RegionList)
        );
        assert_eq!(
            k(
                "map.csv",
                "role,1,2,3\nA,blank,sample,sample\nB,std,std,sample\n"
            ),
            Some(CompanionKind::PlateMap)
        );
        assert_eq!(
            k("README", "about this folder\n"),
            Some(CompanionKind::Document)
        );
        assert_eq!(k("notes.md", "# notes\n"), Some(CompanionKind::Document));
        // data: a numeric export, a header without sheet words, an unknown binary
        let mut data = String::from("time,absorbance\n");
        for i in 0..50 {
            data.push_str(&format!("{i},{}\n", f64::from(i) * 0.01));
        }
        assert_eq!(k("trace.csv", &data), None);
        assert_eq!(k("weird.csv", "alpha,beta\nfoo,bar\n"), None);
        assert_eq!(k("blob.bin", "\u{1}\u{2}"), None);
        assert_eq!(k("empty.csv", ""), None);
    }
}
