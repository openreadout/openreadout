//! What the Agilent chip and tape readers share: gzip-compressed exports, point-to-point sizing
//! against a ladder, and a sample's sized electropherogram as a trace.

use std::collections::BTreeMap;

use openreadout_core::series::{SeriesChannel, SeriesTrace, json_num};
use openreadout_core::{Error, Result};
use serde_json::json;

/// Largest decompressed export read.
pub(crate) const MAX_TEXT: usize = 1 << 30;

/// The bytes of an export, decompressed when they are gzip (`.xml.gz`, `.csv.gz`).
pub(crate) fn plain(bytes: Vec<u8>, format: &'static str) -> Result<Vec<u8>> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let mut out = Vec::new();
        let mut d = flate2::read::MultiGzDecoder::new(&bytes[..]);
        let mut chunk = [0u8; 1 << 16];
        loop {
            let n = std::io::Read::read(&mut d, &mut chunk).map_err(|e| {
                Error::corrupt(format, format!("the gzip stream does not decode: {e}"))
            })?;
            if n == 0 {
                break;
            }
            if out.len() + n > MAX_TEXT {
                return Err(Error::unsupported(
                    format,
                    "an export over 1 GiB decompressed",
                    "No instrument writes exports this large; check that this is the right file.",
                ));
            }
            out.extend_from_slice(&chunk[..n]);
        }
        Ok(out)
    } else {
        Ok(bytes)
    }
}

/// Text of an export: UTF-8 without a byte-order mark, else Latin-1.
pub(crate) fn text(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

/// A calibration curve: (x, size) points sorted by x, duplicates and non-finite points dropped.
#[derive(Debug, Clone, Default)]
pub(crate) struct Curve {
    pub(crate) points: Vec<(f64, f64)>,
    /// Interpolate the logarithm of the size (sizes spread over decades), else the size.
    pub(crate) log: bool,
}

impl Curve {
    pub(crate) fn new(mut pts: Vec<(f64, f64)>) -> Curve {
        pts.retain(|(x, s)| x.is_finite() && s.is_finite());
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.dedup_by(|a, b| a.0 == b.0);
        Curve {
            points: pts,
            log: false,
        }
    }

    /// The same curve, interpolating the logarithm of the size.
    pub(crate) fn log_size(mut self) -> Curve {
        self.points.retain(|p| p.1 > 0.0);
        self.log = true;
        self
    }

    /// The size at `x`, interpolated linearly between the two points around it ("point to
    /// point"); `None` outside the curve.
    pub(crate) fn at(&self, x: f64) -> Option<f64> {
        let p = &self.points;
        if p.len() < 2 || !x.is_finite() {
            return None;
        }
        let k = p.partition_point(|q| q.0 <= x);
        if k == 0 {
            return None;
        }
        if k == p.len() {
            return (x == p[k - 1].0).then_some(p[k - 1].1);
        }
        let (a, b) = (p[k - 1], p[k]);
        let t = (x - a.0) / (b.0 - a.0);
        if !t.is_finite() {
            return None;
        }
        Some(if self.log {
            (a.1.ln() + t * (b.1.ln() - a.1.ln())).exp()
        } else {
            a.1 + t * (b.1 - a.1)
        })
    }

    /// The size range the curve covers.
    pub(crate) fn sizes(&self) -> Option<(f64, f64)> {
        let lo = self
            .points
            .iter()
            .map(|p| p.1)
            .fold(f64::INFINITY, f64::min);
        let hi = self
            .points
            .iter()
            .map(|p| p.1)
            .fold(f64::NEG_INFINITY, f64::max);
        (lo.is_finite() && hi > lo).then_some((lo, hi))
    }
}

/// A sample's electropherogram on a size axis: the samples whose size the curve gives, in rising
/// size order. `x[i]` is where sample `i` sits on the curve's axis, `y` the signal, `own_x` the
/// sample's own position (time in s, distance in %), shown as a third channel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sized_trace(
    name: String,
    curve: &Curve,
    x: &[f64],
    y: &[f64],
    own_x: Option<(&str, &str, &[f64])>,
    size_unit: &str,
    signal: (&str, &str),
    extra: BTreeMap<String, serde_json::Value>,
) -> Option<SeriesTrace> {
    let mut rows: Vec<(f64, f64, f64)> = x
        .iter()
        .zip(y)
        .enumerate()
        .filter_map(|(i, (xv, yv))| {
            let s = curve.at(*xv)?;
            let o = own_x.map_or(f64::NAN, |(_, _, v)| v.get(i).copied().unwrap_or(f64::NAN));
            Some((s, *yv, o))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    // a size axis must rise: keep the first of samples that land on the same size
    rows.dedup_by(|a, b| a.0 <= b.0);
    if rows.len() < 2 {
        return None;
    }
    let mut channels = vec![
        SeriesChannel::new("size", Some(size_unit), "float64"),
        SeriesChannel::new(signal.0, Some(signal.1), "float32"),
    ];
    let mut values = vec![
        rows.iter().map(|r| r.0).collect::<Vec<f64>>(),
        rows.iter().map(|r| r.1).collect(),
    ];
    if let Some((n, u, _)) = own_x {
        channels.push(SeriesChannel::new(n, Some(u), "float64"));
        values.push(rows.iter().map(|r| r.2).collect());
    }
    let mut extra = extra;
    extra.insert(
        "axis".into(),
        json!({"quantity": "size", "unit": size_unit, "irregular": true, "channel": 0, "size": rows.len()}),
    );
    extra.insert("kind".into(), json!("electropherogram"));
    if let Some((lo, hi)) = curve.sizes() {
        extra.insert("sized_range".into(), json!([json_num(lo), json_num(hi)]));
    }
    Some(SeriesTrace {
        name,
        channels,
        sweeps: vec![values],
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_interpolates_point_to_point() {
        let c = Curve::new(vec![
            (30.0, 100.0),
            (10.0, 25.0),
            (20.0, 50.0),
            (20.0, 50.0),
        ]);
        assert_eq!(c.at(15.0), Some(37.5));
        assert_eq!(c.at(25.0), Some(75.0));
        assert_eq!(c.at(30.0), Some(100.0));
        assert_eq!(c.at(5.0), None);
        assert_eq!(c.at(31.0), None);
        assert_eq!(c.sizes(), Some((25.0, 100.0)));
    }

    #[test]
    fn plain_text_passes_and_gzip_inflates() {
        assert_eq!(plain(b"abc".to_vec(), "t").unwrap(), b"abc");
        assert!(plain(vec![0x1f, 0x8b, 0, 0], "t").is_err());
        assert_eq!(text(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(text(b"\xb5l"), "µl");
    }
}
