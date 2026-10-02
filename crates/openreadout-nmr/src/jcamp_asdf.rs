//! Tabular data decoding: ASDF (AFFN, PAC, SQZ, DIF, DUP; JCAMP-DX 4.24 §5) for `(X++(Y..Y))`
//! tables, and AFFN groups for `(XY..XY)` tables. See `docs/formats/jcamp-dx.md`.

use crate::jcamp_parse::parse_affn;
use crate::text::split_comment;

/// One ordinate token of an ASDF line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum YToken {
    /// An actual value (AFFN, PAC or SQZ form).
    Abs(f64),
    /// A difference from the previous value (DIF form).
    Dif(f64),
    /// Repeat the previous token so that it occurs this many times in total (DUP form).
    Dup(u64),
    /// `?`: missing or off-scale ordinate.
    Invalid,
}

/// A lexed data line: the abscissa and its ordinate tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct AsdfLine {
    pub x: f64,
    /// Half a unit in the last written digit of `x` (its rounding uncertainty).
    pub x_resolution: f64,
    pub tokens: Vec<YToken>,
}

/// Most ordinates one table may decode to (a guard against absurd DUP counts in damaged files).
/// 2^24 (128 MiB of `f64`) is far above any 1D spectrum (NMR FIDs reach about 2^20 points;
/// 2D data are split into NTUPLES pages decoded one by one). It was 2^30 (8 GiB) until
/// fuzzing showed a 72-byte table asking for it.
const MAX_VALUES: usize = 1 << 24;

/// A decoded `(X++(Y..Y))` table (values not yet multiplied by the factor).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AsdfTable {
    pub y: Vec<f64>,
    /// `(x as written, its rounding uncertainty, index of the ordinate it labels)` for every
    /// line: the X-sequence check-points.
    pub x_checks: Vec<(f64, f64, usize)>,
    /// `(line within the table, previous ordinate, check value)` where a DIF Y-value check failed.
    pub y_check_failures: Vec<(usize, f64, f64)>,
    /// Data lines read.
    pub lines: usize,
}

fn sqz(c: char) -> Option<i32> {
    match c {
        '@' => Some(0),
        'A'..='I' => Some(c as i32 - 'A' as i32 + 1),
        'a'..='i' => Some(-(c as i32 - 'a' as i32 + 1)),
        _ => None,
    }
}

fn dif(c: char) -> Option<i32> {
    match c {
        '%' => Some(0),
        'J'..='R' => Some(c as i32 - 'J' as i32 + 1),
        'j'..='r' => Some(-(c as i32 - 'j' as i32 + 1)),
        _ => None,
    }
}

fn dup(c: char) -> Option<i32> {
    match c {
        'S'..='Z' => Some(c as i32 - 'S' as i32 + 1),
        's' => Some(9),
        _ => None,
    }
}

fn is_sep(c: char) -> bool {
    c.is_whitespace() || c == ',' || c == ';'
}

/// Read an AFFN number starting at `i` (sign, digits, point, and an exponent `E±dd`).
/// Returns the text and the index after it.
fn affn_at(chars: &[char], mut i: usize) -> (String, usize) {
    let mut s = String::new();
    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
        s.push(chars[i]);
        i += 1;
    }
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
        s.push(chars[i]);
        i += 1;
    }
    // exponent only when E/e is followed by a sign and a digit (otherwise E is SQZ 5, e SQZ -5)
    if i + 2 < chars.len()
        && (chars[i] == 'E' || chars[i] == 'e')
        && (chars[i + 1] == '+' || chars[i + 1] == '-')
        && chars[i + 2].is_ascii_digit()
        && s.bytes().any(|b| b.is_ascii_digit())
    {
        s.push('E');
        s.push(chars[i + 1]);
        i += 2;
        while i < chars.len() && chars[i].is_ascii_digit() {
            s.push(chars[i]);
            i += 1;
        }
    }
    (s, i)
}

fn digits_at(chars: &[char], mut i: usize, s: &mut String) -> usize {
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
        s.push(chars[i]);
        i += 1;
    }
    i
}

/// Lex one data line (comments already removed or not: `$$` is stripped here).
#[allow(clippy::many_single_char_names)]
pub fn lex_asdf_line(line: &str) -> Result<Option<AsdfLine>, String> {
    let (content, _) = split_comment(line);
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    while i < chars.len() && is_sep(chars[i]) {
        i += 1;
    }
    if i >= chars.len() {
        return Ok(None);
    }
    let (xs, next) = affn_at(&chars, i);
    let x = xs
        .parse::<f64>()
        .map_err(|_| format!("line does not start with an abscissa: {content:?}"))?;
    let mantissa = xs.split(['E', 'e']).next().unwrap_or("");
    let decimals = mantissa.split_once('.').map_or(0, |(_, d)| d.len());
    let exp10 = xs
        .split_once(['E', 'e'])
        .and_then(|(_, e)| e.parse::<i32>().ok())
        .unwrap_or(0);
    let x_resolution = 0.5 * 10f64.powi(exp10 - decimals as i32);
    i = next;
    let mut tokens = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if is_sep(c) {
            i += 1;
            continue;
        }
        if c == '?' {
            tokens.push(YToken::Invalid);
            i += 1;
            continue;
        }
        if c == '+' || c == '-' || c == '.' || c.is_ascii_digit() {
            let (s, n) = affn_at(&chars, i);
            let v = s
                .parse::<f64>()
                .map_err(|_| format!("bad number {s:?} in {content:?}"))?;
            tokens.push(YToken::Abs(v));
            i = n;
            continue;
        }
        let (kind, lead) = if let Some(d) = sqz(c) {
            (0, d)
        } else if let Some(d) = dif(c) {
            (1, d)
        } else if let Some(d) = dup(c) {
            (2, d)
        } else {
            return Err(format!("unexpected character {c:?} in {content:?}"));
        };
        let mut s = String::new();
        if lead < 0 {
            s.push('-');
        }
        s.push(char::from_digit(lead.unsigned_abs(), 10).unwrap_or('0'));
        i = digits_at(&chars, i + 1, &mut s);
        match kind {
            0 => tokens.push(YToken::Abs(
                s.parse::<f64>()
                    .map_err(|_| format!("bad SQZ value {s:?}"))?,
            )),
            1 => tokens.push(YToken::Dif(
                s.parse::<f64>()
                    .map_err(|_| format!("bad DIF value {s:?}"))?,
            )),
            _ => tokens.push(YToken::Dup(
                s.parse::<u64>()
                    .map_err(|_| format!("bad DUP count {s:?}"))?,
            )),
        }
    }
    Ok(Some(AsdfLine {
        x,
        x_resolution,
        tokens,
    }))
}

#[derive(Clone, Copy, PartialEq)]
enum Last {
    None,
    Abs(f64),
    Dif(f64),
    Invalid,
}

/// Decode a `(X++(Y..Y))` table body (the lines after the variable list). Check values are
/// integers as written, so they are compared exactly.
#[allow(clippy::float_cmp)]
pub fn decode_asdf(body: &str) -> Result<AsdfTable, String> {
    let mut t = AsdfTable::default();
    let mut last = Last::None;
    let mut ended_dif = false;
    for (n, raw) in body.split('\n').enumerate() {
        let Some(line) = lex_asdf_line(raw.trim_end_matches('\r'))? else {
            continue;
        };
        t.lines += 1;
        let mut toks = line.tokens.as_slice();
        if ended_dif && !toks.is_empty() {
            // Y-value check (§5.8.2): the first ordinate repeats the previous line's last one.
            let prev = t.y.last().copied().unwrap_or(f64::NAN);
            match toks[0] {
                YToken::Abs(v) => {
                    if v != prev && !(v.is_nan() && prev.is_nan()) {
                        t.y_check_failures.push((n, prev, v));
                    }
                    t.x_checks
                        .push((line.x, line.x_resolution, t.y.len().saturating_sub(1)));
                    toks = &toks[1..];
                    // a DUP right after the check value repeats that (absolute) value
                    last = Last::Abs(prev);
                }
                _ => t.x_checks.push((line.x, line.x_resolution, t.y.len())),
            }
        } else {
            t.x_checks.push((line.x, line.x_resolution, t.y.len()));
        }
        for tok in toks {
            match *tok {
                YToken::Abs(v) => {
                    t.y.push(v);
                    last = Last::Abs(v);
                }
                YToken::Invalid => {
                    t.y.push(f64::NAN);
                    last = Last::Invalid;
                }
                YToken::Dif(d) => {
                    let prev =
                        *t.y.last()
                            .ok_or_else(|| "a DIF value has no preceding ordinate".to_string())?;
                    t.y.push(prev + d);
                    last = Last::Dif(d);
                }
                YToken::Dup(k) => {
                    if k == 0 {
                        return Err("DUP count of 0".into());
                    }
                    if usize::try_from(k).map_or(true, |k| t.y.len().saturating_add(k) > MAX_VALUES)
                    {
                        return Err(format!("DUP count {k} is implausibly large"));
                    }
                    for _ in 1..k {
                        match last {
                            Last::None => return Err("a DUP count has no preceding value".into()),
                            Last::Abs(v) => t.y.push(v),
                            Last::Invalid => t.y.push(f64::NAN),
                            Last::Dif(d) => {
                                let prev = *t.y.last().expect("a DIF value was pushed");
                                t.y.push(prev + d);
                            }
                        }
                    }
                }
            }
        }
        ended_dif = !toks.is_empty() && matches!(last, Last::Dif(_));
    }
    Ok(t)
}

/// Decode a `(XY..XY)`-style table body of AFFN groups of `group` components. Components that
/// are not numbers (`?`, `<strings>`, multiplicity letters) become NaN. Returns the components
/// column-wise and the number of non-numeric components seen.
pub fn decode_groups(body: &str, group: usize) -> Result<(Vec<Vec<f64>>, usize), String> {
    if group == 0 {
        return Err("empty variable list".into());
    }
    let mut flat = Vec::new();
    let mut non_numeric = 0usize;
    for raw in body.split('\n') {
        let (content, _) = split_comment(raw);
        let mut rest = content.trim();
        while !rest.is_empty() {
            rest = rest.trim_start_matches(|c: char| is_sep(c));
            if rest.is_empty() {
                break;
            }
            let (tok, after) = if let Some(s) = rest.strip_prefix('<') {
                match s.find('>') {
                    Some(e) => (&rest[..e + 2], &s[e + 1..]),
                    None => (rest, ""),
                }
            } else {
                let e = rest.find(|c: char| is_sep(c)).unwrap_or(rest.len());
                (&rest[..e], &rest[e..])
            };
            rest = after;
            let tok = tok.trim_matches(|c| c == '(' || c == ')');
            if tok.is_empty() {
                continue;
            }
            match parse_affn(tok) {
                Some(v) if tok.len() == affn_len(tok) => flat.push(v),
                _ => {
                    if tok
                        .chars()
                        .any(|c| sqz(c).is_some() || dif(c).is_some() || dup(c).is_some())
                        && tok
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+')
                    {
                        return Err(format!(
                            "ASDF-compressed values in an (XY..XY) table are not decoded: {tok:?}"
                        ));
                    }
                    non_numeric += 1;
                    flat.push(f64::NAN);
                }
            }
        }
    }
    if flat.len() % group != 0 {
        return Err(format!(
            "{} values do not form whole groups of {group}",
            flat.len()
        ));
    }
    let mut cols = vec![Vec::with_capacity(flat.len() / group); group];
    for (i, v) in flat.into_iter().enumerate() {
        cols[i % group].push(v);
    }
    Ok((cols, non_numeric))
}

fn affn_len(tok: &str) -> usize {
    let chars: Vec<char> = tok.chars().collect();
    let (s, end) = affn_at(&chars, 0);
    if s.is_empty() {
        0
    } else {
        chars[..end].iter().map(|c| c.len_utf8()).sum()
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn y(body: &str) -> Vec<f64> {
        decode_asdf(body).unwrap().y
    }

    #[test]
    fn spec_examples() {
        // JCAMP-DX 4.24 Table VIIb: every form encodes 1 2 3 3 2 1 0 -1 -2 -3
        let want = vec![1.0, 2.0, 3.0, 3.0, 2.0, 1.0, 0.0, -1.0, -2.0, -3.0];
        assert_eq!(y("0 1 2 3 3 2 1 0 -1 -2 -3"), want);
        assert_eq!(y("0 1+2+3+3+2+1+0-1-2-3"), want);
        assert_eq!(y("0 1BCCBA@abc"), want);
        assert_eq!(y("0 1JJ%jjjjjj"), want);
        assert_eq!(y("0 1JT%jX"), want);
    }

    #[test]
    fn table_vi_difdup_and_checkpoint() {
        // Table VI.3 of the 4.24 protocol (53 ordinates, DIFDUP, final Y-value check line)
        let body = "599.860@VKT%TLkj%J%KLJ%njKjL%kL%jJULJ%kLK1%lLMNPNPRLJ0QTOJ1P\n700.158A28";
        let t = decode_asdf(body).unwrap();
        assert!(t.y_check_failures.is_empty(), "{:?}", t.y_check_failures);
        assert_eq!(t.y.len(), 53);
        assert_eq!(
            &t.y[..10],
            &[0.0, 0.0, 0.0, 0.0, 2.0, 4.0, 4.0, 4.0, 7.0, 5.0]
        );
        assert_eq!(*t.y.last().unwrap(), 128.0);
    }

    #[test]
    fn affn_exponents_and_sqz_e() {
        assert_eq!(y("1 1.5E+02 -2.0e-01"), vec![150.0, -0.2]);
        // `E` without a sign is SQZ 5
        assert_eq!(y("1 E3e"), vec![53.0, -5.0]);
        let l = lex_asdf_line("16383G6k53 $$ checkpoint").unwrap().unwrap();
        assert_eq!(l.x, 16383.0);
        assert_eq!(l.tokens, vec![YToken::Abs(76.0), YToken::Dif(-253.0)]);
    }

    #[test]
    fn dup_after_check_value_repeats_it() {
        // TESTNTUP.DX writes `h5T`: check value -85, then DUP 2 = one more -85
        assert_eq!(y("1 a0J0\n2 @T"), vec![-10.0, 0.0, 0.0]);
    }

    #[test]
    fn y_check_failure_and_invalid() {
        let t = decode_asdf("1 A J\n3 C").unwrap();
        assert_eq!(t.y, vec![1.0, 2.0]);
        assert_eq!(t.y_check_failures.len(), 1);
        let t = decode_asdf("1 1 ? 3").unwrap();
        assert!(t.y[1].is_nan());
        assert!(decode_asdf("1 J").is_err());
        assert!(decode_asdf("1 A#").is_err());
    }

    #[test]
    fn groups() {
        let (c, bad) = decode_groups("50, 5.84\n51, 9.55; 52,4.19", 2).unwrap();
        assert_eq!(c, vec![vec![50.0, 51.0, 52.0], vec![5.84, 9.55, 4.19]]);
        assert_eq!(bad, 0);
        let (c, bad) = decode_groups("1.0, 2.0, S\n", 3).unwrap();
        assert!(c[2][0].is_nan());
        assert_eq!(bad, 1);
        assert!(decode_groups("1, 2, 3", 2).is_err());
    }
}
