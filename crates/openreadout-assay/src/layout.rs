//! Plate layouts: which well holds what (role, sample, compound, concentration, dilution).
//!
//! Accepted inputs (book/src/guides/plate-analysis.md "Layouts"):
//!
//! - **Plate-map grid** CSV/TSV: one block per field. A block starts with a header line whose
//!   first cell names the field (`role`, `sample`, `concentration`, …) and whose other cells are
//!   the column numbers `1 2 3 …`; the lines below start with the row letter (`A`, `B`, …,
//!   `AA`). Blocks are separated by blank lines; a field name may also stand alone on the line
//!   above the header.
//! - **Long** CSV/TSV: a header line with a `well` column (or `row` and `column`), then one line
//!   per well.
//!
//! Field names are matched case-insensitively with synonyms (`type` → role, `conc`/`dose` →
//! concentration, `drug`/`treatment` → compound, …); other columns are kept as `extra`. The
//! same two shapes are what the sample-sheet/plate-layout work elsewhere in OpenReadout reads;
//! this module is deliberately small so the two can be unified behind [`Layout`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a well is for.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Reagent/buffer blank: its mean is subtracted from every well.
    Blank,
    /// Standard (calibrator) of known concentration.
    Standard,
    /// Unknown sample (or a compound dose in a dose-response).
    Sample,
    /// Positive control (for Z′ and normalisation).
    Positive,
    /// Negative control (for Z′ and normalisation).
    Negative,
    /// A control of unspecified sign.
    Control,
    /// Empty or unused well: ignored.
    Empty,
}

impl Role {
    /// The role id.
    pub fn id(self) -> &'static str {
        match self {
            Role::Blank => "blank",
            Role::Standard => "standard",
            Role::Sample => "sample",
            Role::Positive => "positive",
            Role::Negative => "negative",
            Role::Control => "control",
            Role::Empty => "empty",
        }
    }

    /// Parse a role written in a layout (`blank`, `BLK`, `std`, `unknown`, `pos`, `NC`, …).
    pub fn parse(s: &str) -> Option<Role> {
        let l = norm(s);
        Some(match l.as_str() {
            "blank" | "blk" | "bl" | "blanks" | "buffer" | "bufferonly" | "background" | "bg"
            | "bkg" | "bkgd" | "reagentblank" | "nocell" | "nocells" | "cellfree"
            | "mediumonly" | "mediaonly" => Role::Blank,
            "standard" | "std" | "standards" | "stds" | "calibrator" | "calibrators" | "cal"
            | "calib" | "stdcurve" | "standardcurve" | "s" => Role::Standard,
            "sample" | "samples" | "spl" | "unknown" | "unk" | "un" | "u" | "test" | "x"
            | "smp" | "compound" | "treatment" | "dose" => Role::Sample,
            "positive" | "pos" | "positivecontrol" | "pc" | "posctrl" | "poscontrol" | "posctl"
            | "poscon" | "positivectrl" | "pctrl" | "highcontrol" | "hc" | "max" | "maximum"
            | "maxsignal" | "highsignal" | "high" | "+" => Role::Positive,
            "negative" | "neg" | "negativecontrol" | "nc" | "negctrl" | "negcontrol" | "negctl"
            | "negcon" | "negativectrl" | "nctrl" | "lowcontrol" | "lc" | "min" | "minimum"
            | "minsignal" | "lowsignal" | "low" | "-" => Role::Negative,
            // A vehicle (DMSO, solvent) or untreated well is the no-effect control of an
            // inhibition assay but the full-signal one of an activation assay: its sign is the
            // user's to say (`--role DMSO=negative`).
            "control" | "ctrl" | "ctl" | "qc" | "controls" | "vehicle" | "vehiclecontrol"
            | "dmso" | "dmsocontrol" | "solvent" | "solventcontrol" | "untreated" => Role::Control,
            "empty" | "none" | "unused" | "na" | "n/a" | "" => Role::Empty,
            _ => return None,
        })
    }

    /// Infer a role from a sample name (`STD3`, `Std0001`, `BLK`, `Blank1`, `SPL2:1`,
    /// `Un0004`, `Standard S1`, `Negative control N`, `CTL1`). Unknown names are samples.
    pub fn from_name(name: &str) -> Role {
        let l = norm(name);
        if l.is_empty() {
            return Role::Empty;
        }
        let starts = |p: &str| l.starts_with(p);
        let alnum_after = |p: &str| {
            l.strip_prefix(p)
                .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == ':'))
        };
        if starts("blank") || alnum_after("blk") || alnum_after("bl") || starts("buffer") {
            Role::Blank
        } else if starts("standard")
            || alnum_after("std")
            || starts("calibrator")
            || alnum_after("cal")
            || (l.len() > 1 && alnum_after("s"))
        {
            Role::Standard
        } else if starts("nocell") || starts("background") || starts("mediumonly") {
            Role::Blank
        } else if starts("positive")
            || alnum_after("pos")
            || alnum_after("pc")
            || alnum_after("poscon")
            || alnum_after("posctrl")
            || alnum_after("posctl")
            || starts("poscontrol")
        {
            Role::Positive
        } else if starts("negative")
            || alnum_after("neg")
            || alnum_after("nc")
            || alnum_after("negcon")
            || alnum_after("negctrl")
            || alnum_after("negctl")
            || starts("negcontrol")
        {
            Role::Negative
        } else if starts("control")
            || alnum_after("ctl")
            || alnum_after("ctrl")
            || starts("vehicle")
            || starts("dmso")
            || starts("untreated")
        {
            Role::Control
        } else if matches!(l.as_str(), "empty" | "unused" | "none" | "na" | "-") {
            Role::Empty
        } else {
            Role::Sample
        }
    }
}

/// Lower-case, without spaces, underscores or hyphens (except a lone `-`).
pub(crate) fn norm(s: &str) -> String {
    let t = s.trim();
    if t == "-" || t == "+" {
        return t.to_string();
    }
    t.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// What the layout says about one well.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WellInfo {
    /// Role, explicit or inferred from the sample name.
    pub role: Option<Role>,
    /// The role text as the layout writes it (a `role`/`type` cell), when there is one.
    pub role_label: Option<String>,
    /// The role was set by the user's role map (`--role NAME=ROLE`).
    pub role_mapped: bool,
    /// Sample (replicate-group) name.
    pub sample: Option<String>,
    /// Compound (dose-response series).
    pub compound: Option<String>,
    /// Concentration (standards, doses).
    pub concentration: Option<f64>,
    /// Dilution factor (≥ 1: the sample was diluted this many fold).
    pub dilution: Option<f64>,
    /// Explicit replicate group.
    pub group: Option<String>,
    /// Other columns, verbatim.
    pub extra: BTreeMap<String, String>,
}

impl WellInfo {
    /// Overwrite this well's fields with those set in `other`.
    fn merge(&mut self, other: &WellInfo) {
        if other.role.is_some() {
            self.role = other.role;
            self.role_label.clone_from(&other.role_label);
            self.role_mapped = other.role_mapped;
        }
        for (dst, src) in [
            (&mut self.sample, &other.sample),
            (&mut self.compound, &other.compound),
            (&mut self.group, &other.group),
        ] {
            if src.is_some() {
                dst.clone_from(src);
            }
        }
        if other.concentration.is_some() {
            self.concentration = other.concentration;
        }
        if other.dilution.is_some() {
            self.dilution = other.dilution;
        }
        for (k, v) in &other.extra {
            self.extra.insert(k.clone(), v.clone());
        }
    }

    /// The role: explicit, else from the sample name, else from the compound (a dose), else none.
    pub fn effective_role(&self) -> Option<Role> {
        self.role.or_else(|| {
            self.sample
                .as_deref()
                .map(Role::from_name)
                .or_else(|| self.compound.as_ref().map(|_| Role::Sample))
        })
    }
}

/// A plate layout: well (zero-based row, column) → what it holds.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// Wells the layout names.
    pub wells: BTreeMap<(u32, u32), WellInfo>,
    /// Where each part came from (`embedded:Well ID`, `file:layout.csv`, `--blank-wells`, …).
    pub sources: Vec<String>,
    /// Unit written next to concentrations (`microg/ml`), when one is.
    pub concentration_unit: Option<String>,
}

impl Layout {
    /// Merge `other` into this layout (its fields win).
    pub fn merge(&mut self, other: Layout) {
        for (k, v) in other.wells {
            self.wells.entry(k).or_default().merge(&v);
        }
        self.sources.extend(other.sources);
        if other.concentration_unit.is_some() {
            self.concentration_unit = other.concentration_unit;
        }
    }

    /// Set one field on a list of wells.
    pub fn set(&mut self, wells: &[(u32, u32)], f: impl Fn(&mut WellInfo)) {
        for w in wells {
            f(self.wells.entry(*w).or_default());
        }
    }

    /// Whether any well has an explicit or inferable role.
    pub fn is_empty(&self) -> bool {
        self.wells.is_empty()
    }
}

/// Well name from zero-based row/column: `A1`, `H12`, `AF48`.
pub fn well_name(row: u32, col: u32) -> String {
    format!("{}{}", row_label(row), col + 1)
}

/// Row letters: 0 → `A`, 25 → `Z`, 26 → `AA`.
pub fn row_label(row: u32) -> String {
    let mut n = row + 1;
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push(b'A' + r as u8);
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Zero-based row of a row label (`A` → 0, `AA` → 26); case-insensitive; at most 3 letters.
pub fn parse_row(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() || s.len() > 3 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut n: u32 = 0;
    for c in s.bytes() {
        n = n
            .checked_mul(26)?
            .checked_add(u32::from(c.to_ascii_uppercase() - b'A') + 1)?;
    }
    n.checked_sub(1)
}

/// Parse `A1`, `a01`, `H12`, `AF48` into zero-based (row, column).
pub fn parse_well(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (r, c) = s.split_at(split);
    let row = parse_row(r)?;
    if c.is_empty() || !c.chars().all(|ch| ch.is_ascii_digit()) || c.len() > 4 {
        return None;
    }
    let col: u32 = c.parse().ok()?;
    (col >= 1).then(|| (row, col - 1))
}

/// Parse a well list: `A1,A2`, rectangles `A1:B3` (also `A1-B3`), separated by commas,
/// semicolons or spaces. Rectangles are expanded row-major.
pub fn parse_wells(spec: &str) -> Result<Vec<(u32, u32)>, String> {
    let mut out = Vec::new();
    for part in spec
        .split([',', ';', ' '])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let range = part.split_once(':').or_else(|| part.split_once('-'));
        if let Some((a, b)) = range {
            let (r1, c1) = parse_well(a).ok_or_else(|| format!("`{a}` is not a well name"))?;
            let (r2, c2) = parse_well(b).ok_or_else(|| format!("`{b}` is not a well name"))?;
            let (rl, rh) = (r1.min(r2), r1.max(r2));
            let (cl, ch) = (c1.min(c2), c1.max(c2));
            if u64::from(rh - rl + 1) * u64::from(ch - cl + 1) > 1 << 20 {
                return Err(format!("well range `{part}` is too large"));
            }
            for r in rl..=rh {
                for c in cl..=ch {
                    out.push((r, c));
                }
            }
        } else {
            out.push(parse_well(part).ok_or_else(|| format!("`{part}` is not a well name"))?);
        }
    }
    if out.is_empty() {
        return Err(format!("no wells in `{spec}`"));
    }
    Ok(out)
}

/// Canonical layout field of a column/field name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Field {
    Role,
    Sample,
    Compound,
    Concentration,
    Dilution,
    Group,
    Other(String),
}

fn field_of(name: &str) -> Field {
    let l = norm(name);
    match l.as_str() {
        "role" | "type" | "welltype" | "welltypes" | "kind" | "contenttype" | "wellrole" => {
            Field::Role
        }
        "sample" | "sampleid" | "samplename" | "name" | "id" | "wellid" | "label" | "content"
        | "contents" | "samples" => Field::Sample,
        "compound" | "compoundid" | "compoundname" | "drug" | "treatment" | "inhibitor"
        | "ligand" | "agonist" | "antagonist" => Field::Compound,
        "concentration" | "conc" | "dose" | "standardvalue" | "stdconc" | "amount"
        | "concentrations" => Field::Concentration,
        "dilution" | "dilutionfactor" | "dil" | "df" => Field::Dilution,
        "group" | "replicategroup" | "replicate" | "replicateof" => Field::Group,
        _ => {
            // `concentration (nM)`, `conc [ug/ml]`: a concentration with a unit
            if l.starts_with("concentration") || l.starts_with("conc(") || l.starts_with("conc[") {
                Field::Concentration
            } else {
                Field::Other(name.trim().to_string())
            }
        }
    }
}

/// Unit inside `(…)` or `[…]` of a header (`concentration (nM)` → `nM`).
fn header_unit(name: &str) -> Option<String> {
    let open = name.find(['(', '['])?;
    let close = name[open + 1..].find([')', ']'])? + open + 1;
    let u = name[open + 1..close].trim();
    (!u.is_empty()).then(|| u.to_string())
}

/// Leading number of a cell and the text after it (`1 microg/ml` → (1, `microg/ml`)).
pub fn leading_number(s: &str) -> Option<(f64, String)> {
    let t = s.trim();
    let end = t
        .char_indices()
        .find(|(i, c)| {
            !(c.is_ascii_digit()
                || *c == '.'
                || ((*c == '-' || *c == '+')
                    && (*i == 0 || matches!(t.as_bytes()[i - 1], b'e' | b'E')))
                || ((*c == 'e' || *c == 'E') && *i > 0))
        })
        .map_or(t.len(), |(i, _)| i);
    let v: f64 = t[..end].parse().ok()?;
    v.is_finite().then(|| (v, t[end..].trim().to_string()))
}

/// Dilution factor from `1:10`, `1/10`, `10x`, `10` (a plain number below 1 is a fraction:
/// `0.5` → 2).
pub fn parse_dilution(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Some((a, b)) = t.split_once([':', '/']) {
        let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        return (a > 0.0 && b > 0.0).then(|| b / a);
    }
    let v: f64 = t.trim_end_matches(['x', 'X']).trim().parse().ok()?;
    if !(v.is_finite() && v > 0.0) {
        return None;
    }
    Some(if v < 1.0 { 1.0 / v } else { v })
}

/// Apply one field value to a well.
fn apply(info: &mut WellInfo, field: &Field, value: &str, unit: &mut Option<String>) {
    let v = value.trim();
    if v.is_empty() {
        return;
    }
    match field {
        Field::Role => {
            info.role = Role::parse(v).or(Some(Role::from_name(v)));
            info.role_label = Some(v.to_string());
        }
        Field::Sample => info.sample = Some(v.to_string()),
        Field::Compound => info.compound = Some(v.to_string()),
        Field::Group => info.group = Some(v.to_string()),
        Field::Concentration => {
            if let Some((n, u)) = leading_number(v) {
                info.concentration = Some(n);
                if !u.is_empty() && unit.is_none() {
                    *unit = Some(u);
                }
            } else {
                info.extra
                    .insert("concentration_text".into(), v.to_string());
            }
        }
        Field::Dilution => {
            if let Some(d) = parse_dilution(v) {
                info.dilution = Some(d);
            } else {
                info.extra.insert("dilution_text".into(), v.to_string());
            }
        }
        Field::Other(k) => {
            info.extra.insert(k.clone(), v.to_string());
        }
    }
}

/// Split a delimited line (comma, tab or semicolon), honouring double quotes.
fn split_line(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == delim {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

fn delimiter(text: &str) -> char {
    let head: Vec<&str> = text.lines().take(50).collect();
    let count = |d: char| head.iter().map(|l| l.matches(d).count()).sum::<usize>();
    let (t, c, s) = (count('\t'), count(','), count(';'));
    if t >= c && t >= s && t > 0 {
        '\t'
    } else if s > c {
        ';'
    } else {
        ','
    }
}

/// Whether a header row lists consecutive column numbers `1 2 3 …` after its first cell.
fn column_numbers(cells: &[String]) -> Option<Vec<u32>> {
    let nums: Vec<&String> = cells[1..].iter().take_while(|c| !c.is_empty()).collect();
    if nums.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (i, c) in nums.iter().enumerate() {
        let v: f64 = c.parse().ok()?;
        let want = out
            .first()
            .copied()
            .unwrap_or(v as u32)
            .checked_add(u32::try_from(i).ok()?)?;
        if v.fract() != 0.0 || v < 1.0 || v as u32 != want {
            return None;
        }
        out.push(v as u32);
    }
    Some(out)
}

/// Parse a layout from CSV/TSV text (grid or long form, detected).
pub fn parse_layout(text: &str, source: &str) -> Result<Layout, String> {
    let text = text.trim_start_matches('\u{feff}');
    let delim = delimiter(text);
    let lines: Vec<Vec<String>> = text.lines().map(|l| split_line(l, delim)).collect();
    // long form: a header with a `well` column (or row + column)
    for (i, cells) in lines.iter().enumerate().take(20) {
        let lower: Vec<String> = cells.iter().map(|c| norm(c)).collect();
        let well = lower.iter().position(|c| {
            matches!(
                c.as_str(),
                "well" | "wells" | "wellid" | "position" | "wellposition" | "location"
            )
        });
        let row = lower.iter().position(|c| c == "row");
        let col = lower
            .iter()
            .position(|c| matches!(c.as_str(), "col" | "column"));
        if well.is_some() || (row.is_some() && col.is_some()) {
            // `well id` in Gen5 is a sample name, not a position: prefer a strict `well` column
            let strict = lower.iter().position(|c| {
                matches!(
                    c.as_str(),
                    "well" | "wells" | "position" | "wellposition" | "location"
                )
            });
            return parse_long(&lines[i..], strict.or(well), row, col, source);
        }
    }
    parse_grid(&lines, source)
}

fn parse_long(
    lines: &[Vec<String>],
    well: Option<usize>,
    row: Option<usize>,
    col: Option<usize>,
    source: &str,
) -> Result<Layout, String> {
    let header = &lines[0];
    let fields: Vec<Option<Field>> = header
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if Some(i) == well || Some(i) == row || Some(i) == col || h.is_empty() {
                None
            } else {
                Some(field_of(h))
            }
        })
        .collect();
    let mut layout = Layout {
        sources: vec![source.to_string()],
        ..Layout::default()
    };
    for (i, h) in header.iter().enumerate() {
        if fields.get(i) == Some(&Some(Field::Concentration))
            && let Some(u) = header_unit(h)
        {
            layout.concentration_unit = Some(u);
        }
    }
    let mut unit = layout.concentration_unit.clone();
    for (n, cells) in lines.iter().enumerate().skip(1) {
        if cells.iter().all(String::is_empty) {
            continue;
        }
        let pos = if let Some(w) = well {
            let name = cells.get(w).map_or("", String::as_str);
            parse_well(name)
                .ok_or_else(|| format!("{source}: line {}: `{name}` is not a well name", n + 1))?
        } else {
            let (r, c) = (row.unwrap_or(0), col.unwrap_or(0));
            let rs = cells.get(r).map_or("", String::as_str);
            let cs = cells.get(c).map_or("", String::as_str);
            let rr = parse_row(rs)
                .or_else(|| rs.parse::<u32>().ok().and_then(|v| v.checked_sub(1)))
                .ok_or_else(|| format!("{source}: line {}: bad row `{rs}`", n + 1))?;
            let cc = cs
                .parse::<u32>()
                .ok()
                .and_then(|v| v.checked_sub(1))
                .ok_or_else(|| format!("{source}: line {}: bad column `{cs}`", n + 1))?;
            (rr, cc)
        };
        let info = layout.wells.entry(pos).or_default();
        for (i, f) in fields.iter().enumerate() {
            if let (Some(f), Some(v)) = (f, cells.get(i)) {
                apply(info, f, v, &mut unit);
            }
        }
    }
    if layout.concentration_unit.is_none() {
        layout.concentration_unit = unit;
    }
    if layout.wells.is_empty() {
        return Err(format!("{source}: the layout lists no wells"));
    }
    Ok(layout)
}

fn parse_grid(lines: &[Vec<String>], source: &str) -> Result<Layout, String> {
    let mut layout = Layout {
        sources: vec![source.to_string()],
        ..Layout::default()
    };
    let mut unit = None;
    let mut i = 0;
    let mut blocks = 0;
    while i < lines.len() {
        let cells = &lines[i];
        if cells.len() < 2 {
            i += 1;
            continue;
        }
        let Some(cols) = column_numbers(cells) else {
            i += 1;
            continue;
        };
        // field name: the corner cell, else a lone title on the line above
        let mut name = cells[0].clone();
        if name.is_empty() && i > 0 {
            let above = &lines[i - 1];
            if let Some(t) = above.iter().find(|c| !c.is_empty())
                && above.iter().filter(|c| !c.is_empty()).count() == 1
            {
                name = t.clone();
            }
        }
        if name.is_empty() {
            return Err(format!(
                "{source}: line {}: a plate-map block needs a field name in its corner cell (`role`, `sample`, `concentration`, …)",
                i + 1
            ));
        }
        if let Some(u) = header_unit(&name) {
            unit.get_or_insert(u);
        }
        let field = field_of(&name);
        blocks += 1;
        let mut j = i + 1;
        let mut last_row: Option<u32> = None;
        while j < lines.len() {
            let row_cells = &lines[j];
            let Some(r) = row_cells.first().and_then(|c| parse_row(c)) else {
                break;
            };
            if last_row.is_some_and(|l| r <= l) {
                break;
            }
            last_row = Some(r);
            for (k, c) in cols.iter().enumerate() {
                if let Some(v) = row_cells.get(k + 1)
                    && !v.trim().is_empty()
                {
                    let info = layout.wells.entry((r, c - 1)).or_default();
                    apply(info, &field, v, &mut unit);
                }
            }
            j += 1;
        }
        i = j;
    }
    if blocks == 0 {
        return Err(format!(
            "{source}: no plate-map block (a header of column numbers 1 2 3 …) and no `well` column found"
        ));
    }
    layout.concentration_unit = unit;
    Ok(layout)
}

/// A layout from the maps a plate-reader export embeds (table `extra.layout`: title → well →
/// text): Gen5 `Well ID` + `Conc/Dil`, SkanIt `Sample`, BMG `Content`/`Concentration`, ….
pub fn from_embedded(maps: &serde_json::Value) -> Option<Layout> {
    let obj = maps.as_object()?;
    // a single flat map well → sample name (SkanIt `Sample` matrices)
    if !obj.is_empty() && obj.values().all(serde_json::Value::is_string) {
        let wrapped = serde_json::json!({ "Sample": maps });
        return from_embedded(&wrapped);
    }
    let mut layout = Layout::default();
    let mut unit = None;
    let mut conc_dil: Vec<((u32, u32), String)> = Vec::new();
    for (title, wells) in obj {
        let Some(wells) = wells.as_object() else {
            continue;
        };
        let t = norm(title);
        let field = if t == "conc/dil" || t == "concdil" {
            None
        } else if t == "group" {
            // an assay group (SkanIt `Group 1`), not a replicate group
            Some(Field::Other("assay_group".into()))
        } else {
            match field_of(title) {
                Field::Other(_) if t.contains("layout") || t.contains("content") => {
                    Some(Field::Sample)
                }
                Field::Other(_) if t.contains("concentration") => Some(Field::Concentration),
                Field::Other(_) if t.contains("dilution") => Some(Field::Dilution),
                f => Some(f),
            }
        };
        for (w, v) in wells {
            let Some(pos) = parse_well(w) else { continue };
            let text = v.as_str().map_or_else(|| v.to_string(), str::to_string);
            match &field {
                Some(f) => apply(layout.wells.entry(pos).or_default(), f, &text, &mut unit),
                None => conc_dil.push((pos, text)),
            }
        }
        layout.sources.push(format!("embedded:{title}"));
    }
    // `Conc/Dil` (Gen5, SkanIt): the concentration of standards; a ratio (`1:25`) on other
    // wells is their dilution; other text is kept verbatim (a Gen5 sample's plain number is
    // ambiguous: fraction or factor)
    for (pos, text) in conc_dil {
        let info = layout.wells.entry(pos).or_default();
        if info.effective_role() == Some(Role::Standard) {
            apply(info, &Field::Concentration, &text, &mut unit);
        } else if text.contains(':') && parse_dilution(&text).is_some() {
            info.dilution = parse_dilution(&text);
        } else if !text.trim().is_empty() {
            info.extra
                .insert("conc_dil".into(), text.trim().to_string());
        }
    }
    layout.concentration_unit = unit;
    (!layout.wells.is_empty()).then_some(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wells() {
        assert_eq!(parse_well("A1"), Some((0, 0)));
        assert_eq!(parse_well("h12"), Some((7, 11)));
        assert_eq!(parse_well("AF48"), Some((31, 47)));
        assert_eq!(parse_well("A01"), Some((0, 0)));
        assert_eq!(parse_well("A0"), None);
        assert_eq!(parse_well("12"), None);
        assert_eq!(well_name(31, 47), "AF48");
        assert_eq!(parse_wells("A1:B2, C3").unwrap().len(), 5);
        assert_eq!(parse_wells("H1-H2").unwrap(), vec![(7, 0), (7, 1)]);
        assert!(parse_wells("Q").is_err());
    }

    #[test]
    fn roles_from_names() {
        assert_eq!(Role::from_name("STD3"), Role::Standard);
        assert_eq!(Role::from_name("Std0001"), Role::Standard);
        assert_eq!(Role::from_name("BLK"), Role::Blank);
        assert_eq!(Role::from_name("Blank1"), Role::Blank);
        assert_eq!(Role::from_name("SPL2:1"), Role::Sample);
        assert_eq!(Role::from_name("Un0004"), Role::Sample);
        assert_eq!(Role::from_name("Standard S1"), Role::Standard);
        assert_eq!(Role::from_name("Negative control N"), Role::Negative);
        assert_eq!(Role::from_name("CTL1"), Role::Control);
        assert_eq!(Role::from_name("S1"), Role::Standard);
        assert_eq!(Role::from_name("Sample X1"), Role::Sample);
        assert_eq!(Role::parse("pos"), Some(Role::Positive));
        assert_eq!(Role::parse("Unknown"), Some(Role::Sample));
    }

    #[test]
    fn numbers() {
        assert_eq!(
            leading_number("1 microg/ml"),
            Some((1.0, "microg/ml".into()))
        );
        assert_eq!(leading_number("2.5e-3nM"), Some((0.0025, "nM".into())));
        assert_eq!(leading_number("-1.5"), Some((-1.5, String::new())));
        assert_eq!(parse_dilution("1:10"), Some(10.0));
        assert_eq!(parse_dilution("0.5"), Some(2.0));
        assert_eq!(parse_dilution("25x"), Some(25.0));
    }

    #[test]
    fn grid_layout() {
        let text = "role,1,2,3\nA,standard,standard,sample\nB,blank,blank,sample\n\nconcentration (ng/mL),1,2,3\nA,10,10,\nB,,,\n";
        let l = parse_layout(text, "t").unwrap();
        assert_eq!(l.wells[&(0, 0)].role, Some(Role::Standard));
        assert_eq!(l.wells[&(0, 1)].concentration, Some(10.0));
        assert_eq!(l.wells[&(1, 1)].role, Some(Role::Blank));
        assert_eq!(l.concentration_unit.as_deref(), Some("ng/mL"));
        // title above a bare header
        let text = "sample\n\t1\t2\nA\tS1\tS2\n";
        let l = parse_layout(text, "t").unwrap();
        assert_eq!(l.wells[&(0, 1)].sample.as_deref(), Some("S2"));
    }

    #[test]
    fn long_layout() {
        let text = "Well,Type,Conc,Dilution,Compound,Note\nA1,Std,100,,, first\nA02,Unknown,,1:10,,\nB1,sample,0.3,,Drug X,\n";
        let l = parse_layout(text, "t").unwrap();
        assert_eq!(l.wells[&(0, 0)].role, Some(Role::Standard));
        assert_eq!(l.wells[&(0, 0)].concentration, Some(100.0));
        assert_eq!(l.wells[&(0, 1)].dilution, Some(10.0));
        assert_eq!(l.wells[&(1, 0)].compound.as_deref(), Some("Drug X"));
        assert_eq!(l.wells[&(0, 0)].extra["Note"], "first");
        assert!(parse_layout("Well,Type\nZZZZ9,std\n", "t").is_err());
        assert!(parse_layout("hello\nworld\n", "t").is_err());
    }

    #[test]
    fn embedded_gen5() {
        let maps = serde_json::json!({
            "Well ID": {"A1": "STD1", "A2": "SPL1", "H1": "BLK"},
            "Conc/Dil": {"A1": "2000", "A2": "0.5"}
        });
        let l = from_embedded(&maps).unwrap();
        assert_eq!(l.wells[&(0, 0)].concentration, Some(2000.0));
        assert_eq!(l.wells[&(0, 1)].concentration, None);
        assert_eq!(l.wells[&(0, 1)].extra["conc_dil"], "0.5");
        assert_eq!(l.wells[&(7, 0)].effective_role(), Some(Role::Blank));
    }
}
