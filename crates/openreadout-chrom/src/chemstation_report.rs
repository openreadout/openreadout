//! Peak reports ChemStation writes into a `.D` directory (`docs/formats/chemstation.md`
//! § Vendor peak reports): `Result.xml` (LC/GC ChemStation's XML export: every integrated peak
//! with its limits and baseline, and the compound results), `Report.TXT` (the printed report:
//! one fixed-width table per signal) and `RESULTS.CSV` (MSD ChemStation: the TIC integration).
//! Rows that do not fit the layout are counted, not guessed.

/// Largest report read (the corpus ones are a few kilobytes).
pub const MAX_REPORT_BYTES: u64 = 4 << 20;

/// One peak of a vendor report.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportPeak {
    /// The signal as the report names it (`FID1 A`, `TIC: RAU-R505-1.D\data.ms`).
    pub signal: String,
    /// Peak number within the signal's table.
    pub number: u32,
    pub rt_min: f64,
    /// Peak/baseline type code as written (`BB`, `BB S`, `M2`).
    pub peak_type: String,
    pub width_min: f64,
    pub area: f64,
    pub height: f64,
    pub area_pct: f64,
    /// Compound name and amount (external/internal-standard reports).
    pub name: Option<String>,
    pub amount: f64,
    /// MSD reports: 1-based scan numbers of the peak's start, apex and end.
    pub first_scan: Option<u32>,
    pub max_scan: Option<u32>,
    pub last_scan: Option<u32>,
    /// `Result.xml`: integration limits (min) and the baseline's value at each.
    pub start_min: f64,
    pub end_min: f64,
    pub baseline_start: f64,
    pub baseline_end: f64,
    pub symmetry: f64,
    /// `Result.xml`: the amount's unit (`%`, `% v/v`) and the compound's id in the method.
    pub amount_unit: Option<String>,
    pub compound_id: Option<u32>,
}

impl ReportPeak {
    /// A peak with only its signal, number and retention time known.
    pub fn blank(signal: &str, number: u32, rt_min: f64) -> Self {
        ReportPeak {
            signal: signal.to_string(),
            number,
            rt_min,
            peak_type: String::new(),
            width_min: f64::NAN,
            area: f64::NAN,
            height: f64::NAN,
            area_pct: f64::NAN,
            name: None,
            amount: f64::NAN,
            first_scan: None,
            max_scan: None,
            last_scan: None,
            start_min: f64::NAN,
            end_min: f64::NAN,
            baseline_start: f64::NAN,
            baseline_end: f64::NAN,
            symmetry: f64::NAN,
            amount_unit: None,
            compound_id: None,
        }
    }

    /// True when the peak type carries a flag after its two baseline letters (`BV E`, `VV R`,
    /// `BB S`): skimmed, reconstructed or otherwise special peaks whose baseline is not the
    /// straight line between its limits.
    pub fn flagged(&self) -> bool {
        self.peak_type.chars().skip(2).any(|c| !c.is_whitespace())
    }
}

/// One signal's table of a report.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportTable {
    /// `Report.TXT` or `RESULTS.CSV`.
    pub source: String,
    pub signal: String,
    /// Units the header states for area and height (`pA*s`, `pA`).
    pub area_unit: Option<String>,
    pub height_unit: Option<String>,
    /// Column headings as written.
    pub columns: Vec<String>,
    pub peaks: Vec<ReportPeak>,
    /// Rows that looked like peaks but did not parse.
    pub skipped: usize,
}

fn num(s: &str) -> f64 {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .unwrap_or(f64::NAN)
}

/// UTF-16LE (with BOM) or 8-bit text.
pub fn decode_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

/// Column extents from a separator line of dash runs each closed by `|`: `[start, end)` in
/// characters, the `|` belonging to the column on its left.
fn extents(sep: &str) -> Option<Vec<(usize, usize)>> {
    let chars: Vec<char> = sep.trim_end().chars().collect();
    if chars.is_empty() || chars.iter().any(|c| *c != '-' && *c != '|') || !chars.contains(&'|') {
        return None;
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '|' {
            out.push((start, i + 1));
            start = i + 1;
        }
    }
    (out.len() >= 3).then_some(out)
}

fn cut(line: &[char], (a, b): (usize, usize)) -> String {
    line.get(a.min(line.len())..b.min(line.len()))
        .map(|s| s.iter().collect::<String>().trim().to_string())
        .unwrap_or_default()
}

/// Unit in brackets (`[pA*s]` → `pA*s`).
fn bracket(s: &str) -> Option<String> {
    let a = s.find('[')?;
    let b = s[a..].find(']')? + a;
    Some(s[a + 1..b].trim().to_string()).filter(|u| !u.is_empty())
}

/// Parse `Report.TXT` (text already decoded).
pub fn parse_report_txt(text: &str) -> Vec<ReportTable> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut out = Vec::new();
    let mut signal: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(rest) = l.trim_start().strip_prefix("Signal ")
            && let Some((_, name)) = rest.split_once(':')
        {
            signal = Some(name.split(',').next().unwrap_or("").trim().to_string());
            i += 1;
            continue;
        }
        let Some(ext) = extents(l).filter(|_| i >= 2) else {
            i += 1;
            continue;
        };
        let (h1, h2): (Vec<char>, Vec<char>) = (
            lines[i - 2].chars().collect(),
            lines[i - 1].chars().collect(),
        );
        let columns: Vec<String> = ext
            .iter()
            .map(|e| {
                let (a, b) = (cut(&h1, *e), cut(&h2, *e));
                if b.is_empty() { a } else { format!("{a} {b}") }
            })
            .collect();
        let find = |pred: &dyn Fn(&str) -> bool| columns.iter().position(|c| pred(c));
        let c_num = find(&|c| c.starts_with("Peak") || c == "#");
        let c_rt = find(&|c| c.starts_with("RetTime"));
        let c_type = find(&|c| c.starts_with("Type"));
        let c_width = find(&|c| c.starts_with("Width"));
        let c_pct = find(&|c| c.starts_with("Area") && c.contains('%'));
        let c_area = find(&|c| c.starts_with("Area") && !c.contains('%') && !c.contains('/'));
        let c_height = find(&|c| c.starts_with("Height"));
        let c_amount = find(&|c| c.starts_with("Amount"));
        let c_name = find(&|c| c.starts_with("Name"));
        let mut t = ReportTable {
            source: "Report.TXT".into(),
            signal: signal.clone().unwrap_or_default(),
            area_unit: c_area.and_then(|c| bracket(&columns[c])),
            height_unit: c_height.and_then(|c| bracket(&columns[c])),
            columns: columns.clone(),
            peaks: Vec::new(),
            skipped: 0,
        };
        i += 1;
        while i < lines.len() {
            let row = lines[i];
            if row.trim().is_empty() || row.trim_start().starts_with("Totals") {
                break;
            }
            let chars: Vec<char> = row.chars().collect();
            let get = |c: Option<usize>| c.map(|c| cut(&chars, ext[c])).unwrap_or_default();
            let number = get(c_num).parse::<u32>().ok();
            let rt = num(&get(c_rt));
            match number {
                Some(n) if rt.is_finite() => t.peaks.push(ReportPeak {
                    peak_type: get(c_type),
                    width_min: num(&get(c_width)),
                    area: num(&get(c_area)),
                    height: num(&get(c_height)),
                    area_pct: num(&get(c_pct)),
                    name: Some(get(c_name)).filter(|s| !s.is_empty()),
                    amount: num(&get(c_amount)),
                    ..ReportPeak::blank(&t.signal, n, rt)
                }),
                _ => t.skipped += 1,
            }
            i += 1;
        }
        out.push(t);
    }
    out
}

/// Split a `RESULTS.CSV` value list: comma-separated, `"…"` quoted.
fn csv_fields(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Parse `RESULTS.CSV` (MSD ChemStation): every `[INT …]` section with a `Header=` line.
pub fn parse_results_csv(text: &str) -> Vec<ReportTable> {
    let mut out: Vec<ReportTable> = Vec::new();
    let mut header: Option<Vec<String>> = None;
    for l in text.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some(sec) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            header = None;
            if let Some(sig) = sec.strip_prefix("INT ") {
                out.push(ReportTable {
                    source: "RESULTS.CSV".into(),
                    signal: sig.trim().to_string(),
                    area_unit: None,
                    height_unit: None,
                    columns: Vec::new(),
                    peaks: Vec::new(),
                    skipped: 0,
                });
            }
            continue;
        }
        let Some(t) = out.last_mut() else { continue };
        let Some((key, value)) = l.split_once("=,") else {
            continue;
        };
        if key == "Header" {
            let h: Vec<String> = csv_fields(value)
                .into_iter()
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect();
            t.columns.clone_from(&h);
            header = Some(h);
            continue;
        }
        let (Some(h), Ok(_)) = (&header, key.trim().parse::<u32>()) else {
            continue;
        };
        let f = csv_fields(value);
        let col = |name: &str| h.iter().position(|c| c == name).and_then(|i| f.get(i));
        let int = |name: &str| col(name).and_then(|v| v.trim().parse::<u32>().ok());
        let val = |name: &str| col(name).map_or(f64::NAN, |v| num(v));
        match (int("Peak"), val("R.T.")) {
            (Some(n), rt) if rt.is_finite() => t.peaks.push(ReportPeak {
                peak_type: col("PK TY")
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default(),
                area: val("Area"),
                height: val("Height"),
                area_pct: val("Pct Total"),
                first_scan: int("First"),
                max_scan: int("Max"),
                last_scan: int("Last"),
                ..ReportPeak::blank(&t.signal, n, rt)
            }),
            _ => t.skipped += 1,
        }
    }
    out
}

/// The text of the child element `name`, trimmed (empty → `None`).
fn child_text(n: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn child_num(n: roxmltree::Node<'_, '_>, name: &str) -> f64 {
    child_text(n, name).map_or(f64::NAN, |t| num(&t))
}

/// The signal part of a ChemStation signal description (`FID1 A, Front Signal` → `FID1 A`).
fn signal_of(desc: &str) -> String {
    desc.split(',').next().unwrap_or("").trim().to_string()
}

/// One instrument module that `Result.xml` lists under `ModuleInformation` (`Agilent 7890A`,
/// serial `CN10834060`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultModule {
    /// `ModuleName`: the instrument's product name.
    pub name: Option<String>,
    /// `SerialNumber`.
    pub serial: Option<String>,
    /// `FirmwareRevision`.
    pub firmware: Option<String>,
    /// `PartNumber`.
    pub part_number: Option<String>,
}

/// Parse `text` (a decoded `Result.xml`) and hand its `ChemStationResult` root to `f`.
fn with_result_root<T>(text: &str, f: impl FnOnce(roxmltree::Node<'_, '_>) -> T) -> Option<T> {
    // the declaration says UTF-16; the text is already decoded
    let body = text.trim_start_matches('\u{feff}');
    let body = if body.starts_with("<?xml") {
        body.split_once("?>").map_or(body, |(_, b)| b)
    } else {
        body
    };
    let opts = roxmltree::ParsingOptions {
        allow_dtd: false,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(body, opts).ok()?;
    let root = doc.root_element();
    (root.tag_name().name() == "ChemStationResult").then(|| f(root))
}

/// The instrument modules of a `Result.xml` (`ModuleInformation/Module`), in file order; empty
/// when the text is not a ChemStation result document or lists none.
pub fn parse_result_modules(text: &str) -> Vec<ResultModule> {
    with_result_root(text, |root| {
        root.descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Module")
            .filter(|n| {
                n.parent_element()
                    .is_some_and(|p| p.tag_name().name() == "ModuleInformation")
            })
            .map(|m| ResultModule {
                name: child_text(m, "ModuleName"),
                serial: child_text(m, "SerialNumber"),
                firmware: child_text(m, "FirmwareRevision"),
                part_number: child_text(m, "PartNumber"),
            })
            .filter(|m| m.name.is_some() || m.serial.is_some() || m.part_number.is_some())
            .collect()
    })
    .unwrap_or_default()
}

/// Parse `Result.xml` (text already decoded): every `Chromatograms/Signal` with its
/// `IntegrationResults`, and the `Results/ResultsGroup/Peak` compound results matched to them by
/// signal and retention time. `None` when the text is not a ChemStation result document.
pub fn parse_result_xml(text: &str) -> Option<Vec<ReportTable>> {
    with_result_root(text, result_tables)
}

fn result_tables(root: roxmltree::Node<'_, '_>) -> Vec<ReportTable> {
    let mut out: Vec<ReportTable> = Vec::new();
    for s in root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Signal")
        .filter(|n| {
            n.parent_element()
                .is_some_and(|p| p.tag_name().name() == "Chromatograms")
        })
    {
        let signal = signal_of(&child_text(s, "Description").unwrap_or_default());
        let y = child_text(s, "YUnits");
        let mut t = ReportTable {
            source: "Result.xml".into(),
            signal: signal.clone(),
            area_unit: y.as_ref().map(|u| format!("{u}*s")),
            height_unit: y,
            columns: Vec::new(),
            peaks: Vec::new(),
            skipped: 0,
        };
        for (k, p) in s
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "IntegrationResults")
            .enumerate()
        {
            let rt = child_num(p, "RetTime");
            if !rt.is_finite() {
                t.skipped += 1;
                continue;
            }
            t.peaks.push(ReportPeak {
                area: child_num(p, "Area"),
                height: child_num(p, "Height"),
                width_min: child_num(p, "Width"),
                area_pct: child_num(p, "AreaPercent"),
                symmetry: child_num(p, "Symmetry"),
                start_min: child_num(p, "TimeStart"),
                end_min: child_num(p, "TimeEnd"),
                baseline_start: child_num(p, "BaselineStart"),
                baseline_end: child_num(p, "BaselineEnd"),
                ..ReportPeak::blank(&signal, u32::try_from(k + 1).unwrap_or(u32::MAX), rt)
            });
        }
        out.push(t);
    }
    for p in root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Peak")
        .filter(|n| {
            n.parent_element()
                .is_some_and(|p| p.tag_name().name() == "ResultsGroup")
        })
    {
        let signal = signal_of(&child_text(p, "SignalDesc").unwrap_or_default());
        let rt = child_num(p, "MeasRetTime");
        let Some(t) = out.iter_mut().find(|t| t.signal == signal) else {
            continue;
        };
        let Some(q) = t.peaks.iter_mut().find(|q| (q.rt_min - rt).abs() <= 1e-6) else {
            t.skipped += 1;
            continue;
        };
        q.peak_type = child_text(p, "PeakType").unwrap_or_default();
        q.name = child_text(p, "Name");
        let amount = p
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "Amount");
        q.amount = amount.and_then(|a| a.text()).map_or(f64::NAN, num);
        q.amount_unit = amount
            .and_then(|a| a.attribute("Unit"))
            .map(str::to_string)
            .filter(|u| !u.is_empty());
        q.compound_id = child_text(p, "CompoundID").and_then(|c| c.parse().ok());
    }
    out
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // values parsed from text are compared exactly
mod tests {
    use super::*;

    const REPORT: &str = "Data File C:\\x.D\r\n\r\nSignal 1: FID1 A, \r\n\r\nPeak RetTime Type  Width     Area      Height     Area  \r\n  #   [min]        [min]   [pA*s]      [pA]         %\r\n----|-------|----|-------|----------|----------|--------|\r\n   1   2.824 BB S  0.0881 4.57137e4  7718.08594 97.91525\r\n   2   4.057 BB    0.0928  955.59546  149.11475  2.04681\r\nTotals :                  4.66870e4  7870.60554\r\n\r\nSignal 2: TCD2 B, \r\n\r\nPeak RetTime Type  Width     Area      Height     Area  \r\n  #   [min]        [min]  [25 uV*s]   [25 uV]       %\r\n----|-------|----|-------|----------|----------|--------|\r\n   1   2.828 BB    0.0864 2608.15039  444.79111 1.000e2 \r\nTotals :                  2608.15039  444.79111\r\n";

    #[test]
    fn report_txt_tables() {
        let t = parse_report_txt(REPORT);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].signal, "FID1 A");
        assert_eq!(t[0].area_unit.as_deref(), Some("pA*s"));
        assert_eq!(t[0].height_unit.as_deref(), Some("pA"));
        assert_eq!(t[0].peaks.len(), 2);
        let p = &t[0].peaks[0];
        assert_eq!((p.number, p.peak_type.as_str()), (1, "BB S"));
        assert_eq!(
            (p.rt_min, p.width_min, p.area, p.height, p.area_pct),
            (2.824, 0.0881, 45713.7, 7718.08594, 97.91525)
        );
        assert_eq!(t[1].signal, "TCD2 B");
        assert_eq!(t[1].area_unit.as_deref(), Some("25 uV*s"));
        assert_eq!(t[1].peaks[0].area_pct, 100.0);
        // UTF-16 with BOM
        let mut b = vec![0xFF, 0xFE];
        for u in REPORT.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        // (amount is NaN, so compare the printed tables)
        assert_eq!(
            format!("{:?}", parse_report_txt(&decode_text(&b))),
            format!("{t:?}")
        );
    }

    #[test]
    fn results_csv() {
        let s = "[contents]\r\ncount=1\r\n1=,INT TIC: A.D\\data.ms\r\n[INT TIC: A.D\\data.ms]\r\nTime=,Thu Jun 06 16:30:15 2024\r\nHeader=,\"Peak\",\"R.T.\",\"First\",\"Max\",\"Last\",\"PK  TY\",\"Height\",\"Area\",\"Pct Max\",\"Pct Total\"\r\n1=,  1,  6.056,  857, 860, 873,\"  M \",1861939, 15635678,100.00, 61.453\r\n3=,  3, 10.319, 1595,1598,1604,\"  M2\",  23793,   581160,  3.72,  2.284\r\n";
        let t = parse_results_csv(s);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].signal, "TIC: A.D\\data.ms");
        let p = &t[0].peaks[1];
        assert_eq!(
            (p.number, p.rt_min, p.peak_type.as_str()),
            (3, 10.319, "M2")
        );
        assert_eq!(
            (p.first_scan, p.max_scan, p.last_scan),
            (Some(1595), Some(1598), Some(1604))
        );
        assert_eq!((p.height, p.area, p.area_pct), (23_793.0, 581_160.0, 2.284));
    }

    #[test]
    fn result_xml() {
        let x = r#"<?xml version = "1.0" encoding="utf-16"?>
<ChemStationResult><Chromatograms><Signal><Description>FID1 A, Front Signal</Description><YUnits>pA</YUnits>
<IntegrationResults><RetTime>0.167537</RetTime><Area>685.784119</Area><AreaPercent>0.125136</AreaPercent><Height>260.630585</Height><Width>0.036698</Width><Symmetry>1.573082</Symmetry><TimeStart>0.128731</TimeStart><BaselineStart>2.16862</BaselineStart><TimeEnd>0.194599</TimeEnd><BaselineEnd>28.159346</BaselineEnd></IntegrationResults>
<IntegrationResults><RetTime>0.2</RetTime><Area>10</Area></IntegrationResults></Signal></Chromatograms>
<Results><ResultsGroup><Peak><CompoundID>5</CompoundID><SignalDesc>FID1 A, Front Signal</SignalDesc><PeakType>BV E</PeakType><MeasRetTime Unit="min">0.167537</MeasRetTime><Name>Methane</Name><Amount Unit="% v/v">0.9046673158</Amount></Peak></ResultsGroup></Results></ChemStationResult>"#;
        let t = parse_result_xml(x).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].signal, "FID1 A");
        assert_eq!(t[0].area_unit.as_deref(), Some("pA*s"));
        let p = &t[0].peaks[0];
        assert_eq!(
            (p.rt_min, p.area, p.start_min, p.baseline_end),
            (0.167_537, 685.784_119, 0.128_731, 28.159_346)
        );
        assert_eq!(p.name.as_deref(), Some("Methane"));
        assert_eq!(p.amount_unit.as_deref(), Some("% v/v"));
        assert_eq!(p.compound_id, Some(5));
        assert!(p.flagged());
        assert!(!t[0].peaks[1].flagged());
        assert!(parse_result_xml("<x/>").is_none());
        assert!(parse_result_modules(x).is_empty());
        let m = parse_result_modules(
            "<ChemStationResult><ModuleInformation><Module><Number>1</Number><ModuleName>Agilent 7890A</ModuleName><SerialNumber>CN10834060</SerialNumber><FirmwareRevision>A.01.09</FirmwareRevision><PartNumber>7890A</PartNumber></Module></ModuleInformation></ChemStationResult>",
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name.as_deref(), Some("Agilent 7890A"));
        assert_eq!(m[0].serial.as_deref(), Some("CN10834060"));
        assert_eq!(m[0].part_number.as_deref(), Some("7890A"));
    }

    #[test]
    fn junk_is_not_a_table() {
        assert!(
            parse_report_txt("----\n|||\n-|-|-|\n").is_empty()
                || parse_report_txt("----\n|||\n-|-|-|\n")[0].peaks.is_empty()
        );
        assert!(parse_results_csv("Header=,\"Peak\"\n1=,1,2").is_empty());
        assert_eq!(extents("----|---|--|"), Some(vec![(0, 5), (5, 9), (9, 12)]));
        assert!(extents("-- --|").is_none());
    }
}
