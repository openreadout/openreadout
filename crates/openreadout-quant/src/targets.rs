//! Compound lists for targeted quantitation: one row per compound with how to extract its
//! chromatogram (an m/z, an SRM transition, a detector trace or the default chromatogram) and
//! where to expect it (retention time ± window). Read from CSV/TSV (header row) or JSON (an
//! array of objects, or `{"compounds": [...]}`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::extract::Polarity;

/// One compound of a targeted method.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Compound {
    /// Name, reported in the results.
    pub name: String,
    /// XIC m/z (with `precursor_mz`: a product ion in MS/MS scans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mz: Option<f64>,
    /// SRM/MRM precursor (Q1) m/z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q1: Option<f64>,
    /// SRM/MRM product (Q3) m/z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q3: Option<f64>,
    /// Expected retention time, minutes (absent: the largest peak of the chromatogram).
    #[serde(default, alias = "rt", skip_serializing_if = "Option::is_none")]
    pub rt_min: Option<f64>,
    /// Half-width of the retention-time window, minutes.
    #[serde(default, alias = "window", skip_serializing_if = "Option::is_none")]
    pub rt_window_min: Option<f64>,
    /// XIC tolerance, ppm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ppm: Option<f64>,
    /// XIC tolerance, Da (instead of ppm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub da: Option<f64>,
    /// Scan polarity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polarity: Option<Polarity>,
    /// MS level of the scans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_level: Option<u32>,
    /// Precursor m/z of the MS/MS scans (product-ion XIC).
    #[serde(default, alias = "precursor", skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
    /// Detector trace index (for UV, FID, … quantitation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<u32>,
    /// Channel of that trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
}

/// One result row: a compound in one file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CompoundResult {
    /// The input file.
    pub path: String,
    /// Compound name.
    pub compound: String,
    /// Label of the chromatogram it was measured on.
    pub chromatogram: String,
    /// Expected retention time, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_rt_min: Option<f64>,
    /// Window half-width, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rt_window_min: Option<f64>,
    /// True when a peak was found in the window.
    pub found: bool,
    /// Apex retention time, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rt_min: Option<f64>,
    /// Apex − expected retention time, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rt_shift_min: Option<f64>,
    /// Peak area (signal × `area_time_unit` of the method).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area: Option<f64>,
    /// Peak height above the baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Height / noise σ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr: Option<f64>,
    /// Width at half height, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_half_min: Option<f64>,
    /// USP tailing factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tailing_factor: Option<f64>,
    /// Integration start, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_min: Option<f64>,
    /// Integration end, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_min: Option<f64>,
    /// Baseline code of the peak (`BB`, `BV`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_code: Option<String>,
    /// Area % of the peak within its chromatogram.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_percent: Option<f64>,
    /// Unit of `area`, e.g. `counts·min`, `mAU·s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_unit: Option<String>,
    /// Why nothing was found, or anything else worth knowing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

const COLUMNS: &[(&str, &[&str])] = &[
    (
        "name",
        &["name", "compound", "analyte", "id", "compound_name"],
    ),
    ("mz", &["mz", "m/z", "target_mz", "xic_mz"]),
    ("q1", &["q1", "q1_mz"]),
    ("q3", &["q3", "q3_mz", "product_mz"]),
    (
        "rt_min",
        &[
            "rt",
            "rt_min",
            "retention_time",
            "expected_rt",
            "expected_rt_min",
        ],
    ),
    (
        "rt_window_min",
        &[
            "window",
            "rt_window",
            "rt_window_min",
            "rt_tolerance",
            "window_min",
        ],
    ),
    ("ppm", &["ppm", "tolerance_ppm", "mz_tolerance_ppm"]),
    ("da", &["da", "tolerance_da", "mz_tolerance_da"]),
    ("polarity", &["polarity"]),
    ("ms_level", &["ms_level", "mslevel"]),
    ("precursor_mz", &["precursor", "precursor_mz"]),
    ("trace", &["trace"]),
    ("channel", &["channel"]),
];

fn canonical(header: &str) -> Option<&'static str> {
    let h = header.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    COLUMNS
        .iter()
        .find(|(_, names)| names.contains(&h.as_str()))
        .map(|(c, _)| *c)
}

fn accepted() -> String {
    COLUMNS
        .iter()
        .map(|(c, _)| *c)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Split one CSV/TSV line (double quotes group fields; `""` is a quote).
fn split_line(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delim && !quoted => out.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

fn set_field(c: &mut Compound, col: &str, v: &str, line: usize) -> Result<()> {
    if v.is_empty() {
        return Ok(());
    }
    let f = || -> Result<f64> {
        v.parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| {
                Error::Usage(format!(
                    "compound list line {line}: `{col}` is not a number: `{v}`"
                ))
            })
    };
    let u = || -> Result<u32> {
        v.parse::<u32>().map_err(|_| {
            Error::Usage(format!(
                "compound list line {line}: `{col}` is not a whole number: `{v}`"
            ))
        })
    };
    match col {
        "name" => c.name = v.to_string(),
        "mz" => c.mz = Some(f()?),
        "q1" => c.q1 = Some(f()?),
        "q3" => c.q3 = Some(f()?),
        "rt_min" => c.rt_min = Some(f()?),
        "rt_window_min" => c.rt_window_min = Some(f()?),
        "ppm" => c.ppm = Some(f()?),
        "da" => c.da = Some(f()?),
        "polarity" => c.polarity = Some(v.parse()?),
        "ms_level" => c.ms_level = Some(u()?),
        "precursor_mz" => c.precursor_mz = Some(f()?),
        "trace" => c.trace = Some(u()?),
        "channel" => c.channel = Some(u()?),
        _ => {}
    }
    Ok(())
}

/// Parse a compound list: JSON when the text starts with `[` or `{`, otherwise CSV (or TSV when
/// the header contains a tab). Lines starting with `#` and blank lines are skipped.
pub fn parse_compounds(text: &str) -> Result<Vec<Compound>> {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    let list = if trimmed.starts_with('[') || trimmed.starts_with('{') {
        parse_json(trimmed)?
    } else {
        parse_delimited(trimmed)?
    };
    if list.is_empty() {
        return Err(Error::Usage("the compound list is empty".into()));
    }
    for (i, c) in list.iter().enumerate() {
        check(c, i + 1)?;
    }
    Ok(list)
}

fn parse_json(text: &str) -> Result<Vec<Compound>> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Error::Usage(format!("compound list is not valid JSON: {e}")))?;
    let arr = match &v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(o) => o
            .get("compounds")
            .and_then(|c| c.as_array())
            .cloned()
            .ok_or_else(|| {
                Error::Usage("a JSON compound list is an array or {\"compounds\": [...]}".into())
            })?,
        _ => {
            return Err(Error::Usage(
                "a JSON compound list is an array of objects".into(),
            ));
        }
    };
    let mut out = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| Error::Usage(format!("compound {}: not an object", i + 1)))?;
        let mut c = Compound::default();
        for (k, val) in obj {
            let col = canonical(k).ok_or_else(|| {
                Error::Usage(format!(
                    "compound {}: unknown field `{k}` (accepted: {})",
                    i + 1,
                    accepted()
                ))
            })?;
            let s = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            set_field(&mut c, col, &s, i + 1)?;
        }
        out.push(c);
    }
    Ok(out)
}

fn parse_delimited(text: &str) -> Result<Vec<Compound>> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let Some((_, header)) = lines.next() else {
        return Ok(Vec::new());
    };
    let delim = if header.contains('\t') {
        '\t'
    } else if header.contains(';') && !header.contains(',') {
        ';'
    } else {
        ','
    };
    let cols: Vec<&'static str> = split_line(header, delim)
        .iter()
        .map(|h| {
            canonical(h).ok_or_else(|| {
                Error::Usage(format!(
                    "compound list: unknown column `{h}` (accepted: {})",
                    accepted()
                ))
            })
        })
        .collect::<Result<_>>()?;
    let mut out = Vec::new();
    for (n, line) in lines {
        let mut c = Compound::default();
        for (col, v) in cols.iter().zip(split_line(line, delim)) {
            set_field(&mut c, col, &v, n + 1)?;
        }
        out.push(c);
    }
    Ok(out)
}

fn check(c: &Compound, i: usize) -> Result<()> {
    if c.name.trim().is_empty() {
        return Err(Error::Usage(format!("compound {i}: `name` is required")));
    }
    if c.q1.is_some() != c.q3.is_some() {
        return Err(Error::Usage(format!(
            "compound {i} ({}): give both q1 and q3 for a transition",
            c.name
        )));
    }
    if c.mz.is_some() && c.q1.is_some() {
        return Err(Error::Usage(format!(
            "compound {i} ({}): give either mz or q1/q3, not both",
            c.name
        )));
    }
    if c.ppm.is_some() && c.da.is_some() {
        return Err(Error::Usage(format!(
            "compound {i} ({}): give either ppm or da, not both",
            c.name
        )));
    }
    for (v, what) in [
        (c.mz, "mz"),
        (c.q1, "q1"),
        (c.q3, "q3"),
        (c.ppm, "ppm"),
        (c.da, "da"),
        (c.precursor_mz, "precursor"),
    ] {
        if v.is_some_and(|x| x <= 0.0) {
            return Err(Error::Usage(format!(
                "compound {i} ({}): {what} must be > 0",
                c.name
            )));
        }
    }
    if c.rt_window_min.is_some_and(|w| w <= 0.0) {
        return Err(Error::Usage(format!(
            "compound {i} ({}): the window must be > 0 minutes",
            c.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_tsv_and_json() {
        let csv = "# method\nName,m/z,RT,window,ppm\n\"caffeine, std\",195.0877,5.3,0.2,5\nIS,198.1,5.3,,\n";
        let c = parse_compounds(csv).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "caffeine, std");
        assert_eq!(c[0].mz, Some(195.0877));
        assert_eq!(c[0].rt_window_min, Some(0.2));
        assert_eq!(c[1].rt_window_min, None);
        let tsv = "name\tq1\tq3\trt\nPGE2\t351.2\t271.2\t12.1\n";
        let c = parse_compounds(tsv).unwrap();
        assert_eq!((c[0].q1, c[0].q3), (Some(351.2), Some(271.2)));
        let json = r#"[{"name": "A", "mz": 100.5, "polarity": "negative", "rt": 3}]"#;
        let c = parse_compounds(json).unwrap();
        assert_eq!(c[0].polarity, Some(Polarity::Negative));
        assert_eq!(c[0].rt_min, Some(3.0));
        let c = parse_compounds(r#"{"compounds": [{"name": "B", "trace": 1}]}"#).unwrap();
        assert_eq!(c[0].trace, Some(1));
    }

    #[test]
    fn errors_name_the_problem() {
        let e = parse_compounds("name,mzz\nA,1\n").unwrap_err();
        assert!(e.to_string().contains("mzz"), "{e}");
        assert_eq!(e.exit_code(), 2);
        assert!(parse_compounds("name,mz\nA,abc\n").is_err());
        assert!(parse_compounds("name,q1\nA,100\n").is_err());
        assert!(parse_compounds("mz\n100\n").is_err());
        assert!(parse_compounds("").is_err());
        assert!(parse_compounds("[1]").is_err());
    }
}
