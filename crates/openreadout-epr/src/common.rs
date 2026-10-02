//! What both EPR readers share: axes and the experiment facts of the standard parameters.

use openreadout_core::series::{Facts, json_num};
use openreadout_core::time::{full_year, month_from_abbrev};
use serde_json::{Value, json};

use crate::params::{Descriptor, leading_number};

/// An abscissa.
#[derive(Debug, Clone)]
pub(crate) enum Axis {
    /// `first + i × step`.
    Linear {
        first: f64,
        step: f64,
        quantity: &'static str,
        unit: String,
        label: String,
    },
    /// One value per point (a BES3T `IGD` axis file).
    Listed {
        values: Vec<f64>,
        quantity: &'static str,
        unit: String,
        label: String,
    },
}

impl Axis {
    /// Points numbered 0, 1, 2, … (no axis in the file).
    pub(crate) fn points() -> Axis {
        Axis::Linear {
            first: 0.0,
            step: 1.0,
            quantity: "points",
            unit: String::new(),
            label: String::new(),
        }
    }

    pub(crate) fn quantity(&self) -> &'static str {
        match self {
            Axis::Linear { quantity, .. } | Axis::Listed { quantity, .. } => quantity,
        }
    }

    pub(crate) fn unit(&self) -> Option<&str> {
        match self {
            Axis::Linear { unit, .. } | Axis::Listed { unit, .. } => {
                Some(unit.as_str()).filter(|u| !u.is_empty())
            }
        }
    }

    fn label(&self) -> &str {
        match self {
            Axis::Linear { label, .. } | Axis::Listed { label, .. } => label,
        }
    }

    /// The value of point `i`.
    pub(crate) fn at(&self, i: usize) -> f64 {
        match self {
            Axis::Linear { first, step, .. } => first + step * i as f64,
            Axis::Listed { values, .. } => values.get(i).copied().unwrap_or(f64::NAN),
        }
    }

    /// `extra.axis`: `{quantity, unit, first, step, last, size}` or, for a listed axis,
    /// `{quantity, unit, irregular: true, channel}`.
    pub(crate) fn to_json(&self, n: usize, channel: usize) -> Value {
        let mut o = match self {
            Axis::Linear { first, step, .. } => json!({
                "quantity": self.quantity(),
                "first": json_num(*first),
                "step": json_num(*step),
                "last": json_num(self.at(n.saturating_sub(1))),
                "size": n,
            }),
            Axis::Listed { .. } => json!({
                "quantity": self.quantity(),
                "irregular": true,
                "channel": channel,
                "size": n,
            }),
        };
        if let Some(u) = self.unit() {
            o["unit"] = json!(u);
        }
        if !self.label().is_empty() {
            o["label"] = json!(self.label());
        }
        o
    }
}

/// Our quantity for an axis, from its unit and name.
pub(crate) fn axis_quantity(name: &str, unit: &str) -> &'static str {
    match unit.trim() {
        "G" | "mT" | "T" | "Oe" | "kG" => return "magnetic_field",
        "ns" | "us" | "µs" | "ms" | "s" | "min" => return "time",
        "Hz" | "kHz" | "MHz" | "GHz" => return "frequency",
        "deg" | "°" | "degree" | "degrees" => return "angle",
        "dB" => return "attenuation",
        "K" | "C" | "°C" => return "temperature",
        "W" | "mW" => return "power",
        _ => {}
    }
    let n = name.to_ascii_lowercase();
    if n.contains("field") {
        "magnetic_field"
    } else if n.contains("time") {
        "time"
    } else if n.contains("freq") {
        "frequency"
    } else if n.contains("angle") {
        "angle"
    } else if n.contains("power") {
        "power"
    } else if n.contains("temp") {
        "temperature"
    } else {
        "x"
    }
}

/// `MM/DD/YY` (or `MM/DD/YYYY`) and `HH:MM:SS` as an ISO-8601 local time (no zone: the files do
/// not record one).
pub(crate) fn us_date_time(date: &str, time: Option<&str>) -> Option<String> {
    let date = date.trim();
    // `13-Jun-2018` (WinEPR on some systems): day, month name, year
    let dashed: Vec<&str> = date.split('-').collect();
    let (month, day, p2) = if dashed.len() == 3 {
        let month = month_from_abbrev(dashed[1].trim())?;
        (month, dashed[0].trim().parse::<u32>().ok()?, dashed[2])
    } else {
        let p: Vec<&str> = date.split('/').collect();
        if p.len() != 3 {
            return None;
        }
        (p[0].trim().parse().ok()?, p[1].trim().parse().ok()?, p[2])
    };
    let p = ["", "", p2];
    let mut year: u32 = p[2].trim().parse().ok()?;
    if p[2].trim().len() <= 2 {
        year = full_year(year);
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut iso = format!("{year:04}-{month:02}-{day:02}");
    if let Some(t) = time {
        let q: Vec<u32> = t
            .trim()
            .split(':')
            .map(|x| x.trim().parse::<u32>())
            .collect::<Result<_, _>>()
            .ok()?;
        match q.as_slice() {
            [h, mi, se] if *h < 24 && *mi < 60 && *se < 61 => {
                iso.push_str(&format!("T{h:02}:{mi:02}:{se:02}"));
            }
            [h, mi] if *h < 24 && *mi < 60 => iso.push_str(&format!("T{h:02}:{mi:02}:00")),
            _ => {}
        }
    }
    Some(iso)
}

/// Experiment facts of a BES3T standard-parameter layer (`#SPL`, SI units).
pub(crate) fn epr_facts(f: &mut Facts, d: &Descriptor) {
    f.set("instrument.vendor", "Bruker", "the file format")
        .instrument_kind("CHMO:0002253")
        .set(
            "instrument.software",
            "Xepr",
            "BES3T files are written by Xepr",
        );
    f.set(
        "acquisition.operator",
        d.get("OPER").unwrap_or(""),
        "#SPL OPER",
    );
    f.set("sample.name", d.get("SAMP").unwrap_or(""), "#SPL SAMP");
    f.set(
        "acquisition.comment",
        d.get("CMNT").unwrap_or(""),
        "#SPL CMNT",
    );
    f.set("method.name", d.get("TITL").unwrap_or(""), "#DESC TITL");
    // Xepr writes month/day/year; data sets made by other programs (`DSRC MAN`, e.g. EasySpin's
    // export, which writes day/month/year) are not trusted for the date
    if d.get("DSRC") == Some("EXP")
        && let Some(s) = d.get("DATE").and_then(|dt| us_date_time(dt, d.get("TIME")))
    {
        f.set(
            "acquisition.started_at",
            &s,
            "#SPL DATE and TIME (month/day/year, local time)",
        );
    }
    let technique = match d.get("EXPT") {
        Some("CW") => "CHMO:0000329",
        Some("PLS") => "CHMO:0000330",
        _ => "CHMO:0000328",
    };
    f.technique(technique, "#SPL EXPT");
    let num = |k: &str| d.spl.get(k).and_then(|v| leading_number(v));
    if let Some(v) = num("MWFQ") {
        f.number("microwave_frequency", v / 1e9, "GHz", "#SPL MWFQ (Hz)");
    }
    if let Some(v) = num("MWPW") {
        f.number("microwave_power", v * 1e3, "mW", "#SPL MWPW (W)");
    }
    if let Some(v) = num("B0MA") {
        f.number("modulation_amplitude", v * 1e4, "G", "#SPL B0MA (T)");
    }
    if let Some(v) = num("B0MF") {
        f.number("modulation_frequency", v / 1e3, "kHz", "#SPL B0MF (Hz)");
    }
    if let Some(v) = num("RCAG") {
        f.number("receiver_gain", v, "dB", "#SPL RCAG (dB)");
    }
    if let Some(v) = num("RCTC") {
        f.number("time_constant", v * 1e3, "ms", "#SPL RCTC (s)");
    }
    if let Some(v) = num("SPTP") {
        f.number("conversion_time", v * 1e3, "ms", "#SPL SPTP (s)");
    }
    if let Some(v) = num("A1CT") {
        f.number("center_field", v * 1e4, "G", "#SPL A1CT (T)");
    }
    if let Some(v) = num("A1SW") {
        f.number("sweep_width", v * 1e4, "G", "#SPL A1SW (T)");
    }
    if let Some(v) = num("AVGS") {
        f.plain("scans", v, "#SPL AVGS");
    }
    if let Some(v) = num("STMP") {
        f.number("temperature", v, "K", "#SPL STMP (K)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_quantities() {
        assert_eq!(
            us_date_time("11/26/19", Some("14:08:33")).as_deref(),
            Some("2019-11-26T14:08:33")
        );
        assert_eq!(us_date_time("13/26/19", None), None);
        assert_eq!(axis_quantity("Field", "G"), "magnetic_field");
        assert_eq!(axis_quantity("Time", "ns"), "time");
        assert_eq!(axis_quantity("Whatever", ""), "x");
        let a = Axis::Linear {
            first: 3000.0,
            step: 1.0,
            quantity: "magnetic_field",
            unit: "G".into(),
            label: "Field".into(),
        };
        assert_eq!(a.to_json(11, 0)["last"], json!(3010.0));
    }
}
