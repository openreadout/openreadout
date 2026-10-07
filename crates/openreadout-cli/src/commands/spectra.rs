//! Mass spectrometry: `scans` lists the scan headers of a run (scan number, MS level, retention
//! time, polarity, precursor m/z and charge, isolation window, activation, filter string)
//! without decoding peaks (`openreadout_core::scans`); `spectrum` returns one spectrum's m/z and
//! intensity arrays, picked with `--scan`, `--spectrum` or `--ms-level L --nth K`.

use std::io::Write;
use std::path::{Path, PathBuf};

use openreadout_core::model::SpectrumOutput;
use openreadout_core::reader::SpectrumView;
use openreadout_core::scans::{ScanFilter, scan_list, visit_scans};
use openreadout_core::{Error, Registry, Result, ScanHeader, ScanList};

use crate::output::{emit, fail};

/// Arguments of `scans`.
#[derive(Debug, Clone, clap::Args)]
pub struct ScansArgs {
    /// The mass-spectrometry file (Thermo .raw, mzML/.mzML.gz/mzMLb, mzXML, imzML, Bruker .d,
    /// Agilent .d, Waters .raw, Sciex .wiff, ANDI/MS .cdf, ChemStation .ms).
    pub file: PathBuf,
    /// Run index, for files with more than one run (Sciex samples).
    #[arg(long, default_value_t = 0)]
    pub run: u32,
    /// Only scans of this MS level (1 = full scans, 2 = MS/MS).
    #[arg(long, value_name = "N")]
    pub ms_level: Option<u32>,
    /// Only `positive` or `negative` scans.
    #[arg(long, value_name = "POLARITY")]
    pub polarity: Option<String>,
    /// Retention-time window in minutes, `START:END` (either side may be empty: `5:`, `:12.5`;
    /// `START-END` also works).
    #[arg(long, value_name = "START:END", allow_hyphen_values = true)]
    pub rt_range: Option<String>,
    /// Only MS/MS scans whose precursor m/z is within the tolerance of this value.
    #[arg(long, value_name = "MZ")]
    pub precursor: Option<f64>,
    /// Precursor tolerance in m/z units (default 0.01).
    #[arg(long, value_name = "DA", conflicts_with = "precursor_ppm")]
    pub precursor_tol: Option<f64>,
    /// Precursor tolerance in ppm instead.
    #[arg(long, value_name = "PPM")]
    pub precursor_ppm: Option<f64>,
    /// Only precursors of this charge state.
    #[arg(long, value_name = "Z", allow_hyphen_values = true)]
    pub charge: Option<i32>,
    /// Only this activation: `HCD`, `CID`, `ETD`, ... (case-insensitive).
    #[arg(long, value_name = "METHOD")]
    pub activation: Option<String>,
    /// Only scans whose filter string contains this text (case-insensitive).
    #[arg(long, value_name = "TEXT")]
    pub scan_filter: Option<String>,
    /// Skip this many matching scans (paging).
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
    /// List at most this many matching scans (all are still counted). Default 50, or every
    /// match with `--csv`.
    #[arg(long, value_name = "N")]
    pub limit: Option<u64>,
    /// Only count the matching scans (per MS level); list none.
    #[arg(long)]
    pub count: bool,
    /// Write the matching scans as CSV to standard output (one row per scan, streamed).
    #[arg(long, conflicts_with = "json")]
    pub csv: bool,
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `spectrum`.
#[derive(Debug, Clone, clap::Args)]
#[command(group(clap::ArgGroup::new("which").required(true).args(["scan", "spectrum", "nth"])))]
pub struct SpectrumArgs {
    /// The mass-spectrometry file (Thermo .raw, mzML/.mzML.gz/mzMLb, mzXML, imzML, Bruker .d,
    /// Agilent .d, Waters .raw, Sciex .wiff, ANDI/MS .cdf, ChemStation .ms).
    pub file: PathBuf,
    /// Run index, for files with more than one run (Sciex samples).
    #[arg(long, default_value_t = 0)]
    pub run: u32,
    /// This scan number as the instrument counts it (1-based in Thermo files).
    #[arg(long, value_name = "N", conflicts_with_all = ["spectrum", "nth"])]
    pub scan: Option<u64>,
    /// This zero-based spectrum index.
    #[arg(long, value_name = "I", conflicts_with = "nth")]
    pub spectrum: Option<u64>,
    /// With `--ms-level`: the K-th spectrum (from 1) of that level (MS1 and MS/MS scans are
    /// interleaved; `--ms-level 2 --nth 1` is the first MS/MS scan).
    #[arg(long, value_name = "K", requires = "ms_level")]
    pub nth: Option<u64>,
    /// The MS level `--nth` counts in.
    #[arg(long, value_name = "N", requires = "nth")]
    pub ms_level: Option<u32>,
    /// The instrument's stored centroid list instead of the profile when a scan has both.
    #[arg(long)]
    pub centroid: bool,
    /// Leave out the peaks the instrument flags as reference or background ions (Thermo .raw,
    /// listed in `extra.flagged_peaks` otherwise), as ThermoRawFileParser and ProteoWizard
    /// before 3.0.20286 do.
    #[arg(long)]
    pub exclude_flagged: bool,
    /// At most this many points (`point_count` still reports the full size).
    #[arg(long, value_name = "N")]
    pub max_points: Option<usize>,
    #[arg(long)]
    pub json: bool,
}

/// Default page size of `scans` (JSON and text).
pub const DEFAULT_LIMIT: u64 = 50;

/// `--rt-range START:END` (or `START-END`) in minutes → seconds (`None` for an open side).
pub fn parse_rt_minutes(s: &str) -> Result<(Option<f64>, Option<f64>)> {
    let bad = || {
        Error::Usage(format!(
            "--rt-range {s}: give START:END in minutes, e.g. 5:12.5, 5: or :12.5"
        ))
    };
    // `START:END`; else `START-END`, where a leading '-' is an open start and the separator
    // is the first '-' after position 0.
    let cut = match s.find(':') {
        Some(i) => i,
        None => s
            .char_indices()
            .skip(1)
            .find(|&(i, c)| c == '-' && !s[..i].ends_with(['e', 'E']))
            .map(|(i, _)| i)
            .or_else(|| s.starts_with('-').then_some(0))
            .ok_or_else(bad)?,
    };
    let (a, b) = (s[..cut].trim(), s[cut + 1..].trim());
    let num = |t: &str| -> Result<Option<f64>> {
        if t.is_empty() {
            Ok(None)
        } else {
            t.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| Some(v * 60.0))
                .ok_or_else(bad)
        }
    };
    Ok((num(a)?, num(b)?))
}

impl ScansArgs {
    /// The conditions the arguments set.
    pub fn filter(&self) -> Result<ScanFilter> {
        let (rt_min_s, rt_max_s) = match &self.rt_range {
            Some(r) => parse_rt_minutes(r)?,
            None => (None, None),
        };
        let f = ScanFilter {
            ms_level: self.ms_level,
            polarity: self.polarity.as_ref().map(|p| p.to_ascii_lowercase()),
            rt_min_s,
            rt_max_s,
            precursor_mz: self.precursor,
            precursor_tol_mz: self.precursor_tol,
            precursor_tol_ppm: self.precursor_ppm,
            charge: self.charge,
            activation: self.activation.clone(),
            filter_contains: self.scan_filter.clone(),
        };
        f.validate()?;
        Ok(f)
    }
}

/// List the scans of `file` (the JSON of `scans --json`).
pub fn scans(
    reg: &Registry,
    file: &Path,
    run: u32,
    filter: &ScanFilter,
    offset: u64,
    limit: u64,
) -> Result<ScanList> {
    let (det, mut ds) = reg.open(file)?;
    scan_list(
        ds.as_mut(),
        &file.display().to_string(),
        det.format_id,
        run,
        filter,
        offset,
        limit,
    )
}

/// CSV columns of `scans --csv`.
pub const CSV_COLUMNS: &[&str] = &[
    "index",
    "scan_number",
    "native_id",
    "ms_level",
    "rt_s",
    "rt_min",
    "polarity",
    "centroided",
    "precursor_mz",
    "precursor_charge",
    "precursor_intensity",
    "isolation_lower_mz",
    "isolation_upper_mz",
    "activation",
    "collision_energy",
    "inverse_reduced_mobility",
    "scan_filter",
    "scan_window_lower_mz",
    "scan_window_upper_mz",
    "total_ion_current",
    "base_peak_mz",
    "base_peak_intensity",
    "point_count",
];

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// One CSV row of a scan header, in [`CSV_COLUMNS`] order.
pub fn csv_row(h: &ScanHeader) -> String {
    let f = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_default();
    let cols = [
        h.index.to_string(),
        h.scan_number.to_string(),
        csv_field(h.native_id.as_deref().unwrap_or_default()),
        h.ms_level.to_string(),
        f(h.rt_s),
        f(h.rt_s.map(|t| t / 60.0)),
        h.polarity.clone(),
        h.centroided.to_string(),
        f(h.precursor_mz),
        h.precursor_charge
            .map(|z| z.to_string())
            .unwrap_or_default(),
        f(h.precursor_intensity),
        f(h.isolation_window_mz.map(|w| w[0])),
        f(h.isolation_window_mz.map(|w| w[1])),
        h.activation.clone().unwrap_or_default(),
        f(h.collision_energy),
        f(h.inverse_reduced_mobility),
        csv_field(h.scan_filter.as_deref().unwrap_or_default()),
        f(h.scan_window_mz.map(|w| w[0])),
        f(h.scan_window_mz.map(|w| w[1])),
        f(h.total_ion_current),
        f(h.base_peak_mz),
        f(h.base_peak_intensity),
        h.point_count.map(|n| n.to_string()).unwrap_or_default(),
    ];
    cols.join(",")
}

fn run_csv(reg: &Registry, a: &ScansArgs, filter: &ScanFilter) -> Result<()> {
    let (_, mut ds) = reg.open(&a.file)?;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let io = |e: std::io::Error| Error::io(Path::new("<stdout>"), e);
    writeln!(out, "{}", CSV_COLUMNS.join(",")).map_err(io)?;
    let limit = a.limit.unwrap_or(u64::MAX);
    let (mut matched, mut written) = (0u64, 0u64);
    let mut failure = None;
    visit_scans(ds.as_mut(), a.run, &mut |h| {
        if !filter.matches(&h) {
            return true;
        }
        matched += 1;
        if matched <= a.offset {
            return true;
        }
        if written >= limit {
            return false;
        }
        if let Err(e) = writeln!(out, "{}", csv_row(&h)) {
            failure = Some(e);
            return false;
        }
        written += 1;
        true
    })?;
    if let Some(e) = failure {
        // A closed pipe (`| head`) is not an error.
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(io(e));
        }
        return Ok(());
    }
    match out.flush() {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(io(e)),
        _ => Ok(()),
    }
}

fn fmt_opt(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "-".into(), |v| format!("{v:.digits$}"))
}

fn render(o: &ScanList) -> String {
    let counts: Vec<String> = o
        .ms_level_counts
        .iter()
        .map(|(l, n)| format!("MS{l}: {n}"))
        .collect();
    let mut s = format!(
        "{} ({}), run {}: {} of {} scans match ({}){}\n",
        o.path,
        o.format,
        o.run,
        o.matched,
        o.scan_count,
        if counts.is_empty() {
            "none".into()
        } else {
            counts.join(", ")
        },
        if o.source == "spectra" {
            "; listed by decoding each spectrum (no header index)"
        } else {
            ""
        }
    );
    if o.scans.is_empty() {
        return s.trim_end().to_string();
    }
    s.push_str(&format!(
        "{:>7} {:>7} {:>3} {:>9} {:>4} {:>11} {:>3} {:>6} {:>6}  filter\n",
        "index", "scan", "ms", "rt_min", "pol", "precursor", "z", "act", "ce"
    ));
    for h in &o.scans {
        s.push_str(&format!(
            "{:>7} {:>7} {:>3} {:>9} {:>4} {:>11} {:>3} {:>6} {:>6}  {}\n",
            h.index,
            h.scan_number,
            h.ms_level,
            fmt_opt(h.rt_s.map(|t| t / 60.0), 4),
            match h.polarity.as_str() {
                "positive" => "+",
                "negative" => "-",
                _ => "?",
            },
            fmt_opt(h.precursor_mz, 4),
            h.precursor_charge
                .map_or_else(|| "-".into(), |z| z.to_string()),
            h.activation.as_deref().unwrap_or("-"),
            fmt_opt(h.collision_energy, 1),
            h.scan_filter.as_deref().unwrap_or("")
        ));
    }
    if o.truncated {
        s.push_str(&format!(
            "… {} more (--offset {} --limit N, or --csv for all)\n",
            o.matched - o.offset - o.returned,
            o.offset + o.returned
        ));
    }
    s.trim_end().to_string()
}

/// `openreadout scans`.
pub fn run_scans(reg: &Registry, a: &ScansArgs) -> i32 {
    let filter = match a.filter() {
        Ok(f) => f,
        Err(e) => return fail(a.json, &e),
    };
    if a.csv {
        return match run_csv(reg, a, &filter) {
            Ok(()) => 0,
            Err(e) => fail(false, &e),
        };
    }
    let limit = if a.count {
        0
    } else {
        a.limit.unwrap_or(DEFAULT_LIMIT)
    };
    match scans(reg, &a.file, a.run, &filter, a.offset, limit) {
        Ok(v) => emit(a.json, &v, render),
        Err(e) => fail(a.json, &e),
    }
}

/// `openreadout spectrum --scan N | --spectrum I | --ms-level L --nth K`: one spectrum.
pub fn run_spectrum(reg: &Registry, a: &SpectrumArgs) -> i32 {
    let by = match (a.nth, a.scan, a.spectrum) {
        (Some(k), _, _) => SpectrumBy::Level(a.ms_level.unwrap_or(1), k),
        (None, _, Some(i)) => SpectrumBy::Index(i),
        (None, Some(n), None) => SpectrumBy::Scan(n),
        (None, None, None) => {
            return fail(
                a.json,
                &Error::Usage("give --scan N, --spectrum I or --ms-level L --nth K".into()),
            );
        }
    };
    match spectrum(
        reg,
        &a.file,
        by,
        a.run,
        a.centroid,
        a.exclude_flagged,
        a.max_points,
    ) {
        Ok(v) => emit(a.json, &v, render_spectrum),
        Err(e) => fail(a.json, &e),
    }
}

/// How `spectrum` picks one spectrum.
pub enum SpectrumBy {
    Scan(u64),
    Index(u64),
    /// MS level and 1-based rank within that level.
    Level(u32, u64),
}

/// Read one spectrum.
pub fn spectrum(
    reg: &Registry,
    file: &Path,
    by: SpectrumBy,
    run: u32,
    centroid: bool,
    exclude_flagged: bool,
    max_points: Option<usize>,
) -> Result<SpectrumOutput> {
    let (det, mut ds) = reg.open(file)?;
    if exclude_flagged && !ds.exclude_flagged_peaks(true) {
        crate::output::warn(&format!(
            "--exclude-flagged: {} files flag no peaks; the spectrum is unchanged",
            det.format_id
        ));
    }
    let view = if centroid {
        SpectrumView::Centroid
    } else {
        SpectrumView::Primary
    };
    let mut sp = match by {
        SpectrumBy::Index(i) => ds.read_spectrum_view(run, i, view)?,
        SpectrumBy::Scan(n) => {
            openreadout_core::reader::spectrum_by_scan(ds.as_mut(), run, n, view)?
        }
        SpectrumBy::Level(level, nth) => {
            let count = ds
                .info()?
                .spectra
                .iter()
                .find(|s| s.index == run)
                .map_or(0, |s| s.scan_count);
            openreadout_core::reader::spectrum_by_level(ds.as_mut(), run, level, nth, count, view)?
        }
    };
    let point_count = sp.mz.len() as u64;
    let mut truncated = false;
    if let Some(k) = max_points
        && sp.mz.len() > k
    {
        sp.mz.truncate(k);
        sp.intensity.truncate(k);
        truncated = true;
    }
    Ok(SpectrumOutput {
        path: file.display().to_string(),
        format: det.format_id.into(),
        run,
        view: if centroid { "centroid" } else { "primary" }.into(),
        point_count,
        truncated,
        spectrum: sp,
    })
}

pub fn render_spectrum(o: &SpectrumOutput) -> String {
    let s = &o.spectrum;
    let mut out = format!(
        "{} ({}): scan {} (index {})  ms{}  rt {}  {}  {}  {} points{}\n",
        o.path,
        o.format,
        s.scan_number,
        s.index,
        s.ms_level,
        s.rt_s
            .map_or_else(|| "none stated".into(), |t| format!("{t:.3} s")),
        s.polarity,
        if s.centroided { "centroid" } else { "profile" },
        o.point_count,
        if o.truncated { " (truncated)" } else { "" }
    );
    if let Some(f) = &s.scan_filter {
        out.push_str(&format!("  filter: {f}\n"));
    }
    if let Some(p) = s.precursor_mz {
        out.push_str(&format!(
            "  precursor: {p:.4}{}\n",
            s.precursor_charge
                .map(|c| format!(" charge {c}"))
                .unwrap_or_default()
        ));
    }
    if let Some(t) = s.total_ion_current {
        out.push_str(&format!("  total ion current: {t}\n"));
    }
    let shown = s.mz.len().min(20);
    for (m, i) in s.mz.iter().zip(&s.intensity).take(shown) {
        out.push_str(&format!("  {m:>14.6}  {i:>14.2}\n"));
    }
    if s.mz.len() > shown {
        out.push_str(&format!(
            "  ... {} more points (use --json)\n",
            s.mz.len() - shown
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rt_windows_in_minutes() {
        assert_eq!(
            parse_rt_minutes("5-12.5").unwrap(),
            (Some(300.0), Some(750.0))
        );
        assert_eq!(parse_rt_minutes("5-").unwrap(), (Some(300.0), None));
        assert_eq!(parse_rt_minutes("-12.5").unwrap(), (None, Some(750.0)));
        assert_eq!(
            parse_rt_minutes("1e-1-2").unwrap(),
            (Some(6.0), Some(120.0))
        );
        assert_eq!(
            parse_rt_minutes("5:12.5").unwrap(),
            (Some(300.0), Some(750.0))
        );
        assert_eq!(parse_rt_minutes(":12.5").unwrap(), (None, Some(750.0)));
        assert!(parse_rt_minutes("abc").is_err());
        assert!(parse_rt_minutes("5").is_err());
    }

    #[test]
    fn csv_quotes_filters() {
        let h = ScanHeader {
            index: 3,
            scan_number: 4,
            ms_level: 2,
            rt_s: Some(90.0),
            polarity: "positive".into(),
            precursor_mz: Some(445.12),
            precursor_charge: Some(2),
            scan_filter: Some("FTMS + c ESI d Full ms2 445.12@hcd30.00 [100.00-1000.00], x".into()),
            ..ScanHeader::default()
        };
        let row = csv_row(&h);
        assert!(row.split(',').count() >= CSV_COLUMNS.len());
        assert!(row.starts_with("3,4,,2,90,1.5,positive,false,445.12,2,"));
        assert!(row.contains("\"FTMS + c ESI d Full ms2 445.12@hcd30.00 [100.00-1000.00], x\""));
    }
}
