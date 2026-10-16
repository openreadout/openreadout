//! Helpers shared by the readers: number formatting and small geometry.

use serde_json::Value;

/// A finite float as JSON, rounded to 15 significant digits (no binary noise); `null` for
/// NaN and infinities.
pub(crate) fn num(v: f64) -> Value {
    if v.is_finite() {
        let r = openreadout_core::model::round_noise(v);
        if r.fract() == 0.0 && r.abs() < 9.0e15 {
            Value::from(r as i64)
        } else {
            Value::from(r)
        }
    } else {
        Value::Null
    }
}

/// Median of `v` (sorted copy); `None` when empty.
pub(crate) fn median(v: &[f64]) -> Option<f64> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return None;
    }
    s.sort_by(f64::total_cmp);
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2]
    } else {
        f64::midpoint(s[n / 2 - 1], s[n / 2])
    })
}

/// Cross product of two 3-vectors.
pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Dot product of two 3-vectors.
pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The anatomical plane a slice normal (in patient coordinates, LPS or RAS alike) is closest
/// to: `axial` (along the head-foot axis), `coronal` (front-back) or `sagittal` (left-right);
/// `oblique` when no axis dominates (cosine below 0.8).
pub(crate) fn plane_of_normal(n: [f64; 3]) -> &'static str {
    let a = n.map(f64::abs);
    let (i, m) = a
        .iter()
        .copied()
        .enumerate()
        .fold((0, 0.0), |b, (i, v)| if v > b.1 { (i, v) } else { b });
    if m < 0.8 {
        return "oblique";
    }
    match i {
        0 => "sagittal",
        1 => "coronal",
        _ => "axial",
    }
}

/// The letters of the patient axes a direction points to most, LPS convention (DICOM):
/// `+x` = L, `+y` = P, `+z` = S (head). Used to say which way rows and columns run.
pub(crate) fn lps_letter(v: [f64; 3]) -> char {
    let a = v.map(f64::abs);
    let i = if a[0] >= a[1] && a[0] >= a[2] {
        0
    } else if a[1] >= a[2] {
        1
    } else {
        2
    };
    let pos = v[i] >= 0.0;
    match (i, pos) {
        (0, true) => 'L',
        (0, false) => 'R',
        (1, true) => 'P',
        (1, false) => 'A',
        (_, true) => 'S',
        (_, false) => 'I',
    }
}
