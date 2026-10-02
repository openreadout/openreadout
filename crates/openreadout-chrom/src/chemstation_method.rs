//! The method report ChemStation writes into a `.D` directory as `acqmeth.txt`
//! (`docs/formats/chemstation.md` § Acquisition method text): method path, GC oven program,
//! injection, inlet, column and MS acquisition settings. Lines that do not match are ignored.

use serde_json::{Map, Value, json};

/// Largest `acqmeth.txt` read (the corpus one is 7 KB).
pub const MAX_METHOD_TEXT: u64 = 1 << 20;

/// Words of a line (runs of spaces collapsed).
fn words(l: &str) -> Vec<&str> {
    l.split_whitespace().collect()
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `°C` with or without the degree sign as decoded (`°C`, `C`).
fn is_celsius(w: &str) -> bool {
    w == "\u{b0}C" || w == "C" || w.ends_with("\u{b0}C")
}

/// `60 °C for 1 min` → start step; `then 10 °C/min to 325 °C for 10 min` → ramp step.
fn oven_step(line: &str) -> Option<Value> {
    let tokens = words(line);
    match tokens.as_slice() {
        [temp, unit, "for", hold, "min"] if is_celsius(unit) => Some(json!({
            "temperature_c": num(temp)?, "hold_min": num(hold)?,
        })),
        [
            "then",
            rate,
            rate_unit,
            "to",
            temp,
            unit,
            "for",
            hold,
            "min",
        ] if rate_unit.ends_with("C/min") && is_celsius(unit) => Some(json!({
            "rate_c_per_min": num(rate)?, "temperature_c": num(temp)?, "hold_min": num(hold)?,
        })),
        _ => None,
    }
}

/// Parse `acqmeth.txt`. `None` when nothing recognizable is in it.
pub fn parse_acqmeth(text: &str) -> Option<Value> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Map::new();
    let mut put = |k: &str, v: Value| {
        out.entry(k.to_string()).or_insert(v);
    };
    let mut section = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        let tokens = words(line);
        if let Some(rest) = line.strip_prefix("INSTRUMENT CONTROL PARAMETERS:") {
            put("instrument_name", json!(rest.trim()));
        } else if tokens.len() == 1
            && (line.contains(":\\") || line.starts_with("\\\\"))
            && line.to_ascii_uppercase().ends_with(".M")
        {
            put("method_path", json!(line));
        } else if let Some(sn) = line
            .strip_prefix("TUNE PARAMETERS for SN:")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            put("ms_serial", json!(sn));
        } else if tokens == ["Oven", "Program", "On"] {
            let mut steps = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                match oven_step(lines[j]) {
                    Some(s) => steps.push(s),
                    None if lines[j].trim().is_empty() => {}
                    None => break,
                }
                j += 1;
            }
            if !steps.is_empty() {
                put("oven_program", Value::Array(steps));
            }
            if let Some(["Run", "Time", t, "min"]) = lines.get(j).map(|line| words(line)).as_deref()
                && let Some(t) = num(t)
            {
                put("run_time_min", json!(t));
            }
            i = j;
            continue;
        } else if let ["Injection", "Volume", v, u] = tokens.as_slice()
            && (*u == "\u{b5}L" || *u == "uL")
            && let Some(v) = num(v)
        {
            put("injection_volume_ul", json!(v));
        } else if let ["Mode", m] = tokens.as_slice()
            && section.contains("Inlet")
        {
            put("inlet_mode", json!(m));
        } else if line.starts_with("Column #") {
            let name = lines[i + 1..]
                .iter()
                .map(|x| x.trim())
                .find(|x| !x.is_empty() && !x.contains(':'));
            let dims = lines[i + 1..]
                .iter()
                .take(4)
                .filter_map(|x| x.rsplit_once(':').map(|(_, d)| d.trim()))
                .find(|d| d.contains(" x ") && d.contains(" m "));
            if let Some(n) = name {
                put(
                    "column",
                    json!(match dims {
                        Some(d) => format!("{n}, {d}"),
                        None => n.to_string(),
                    }),
                );
            }
        } else if let Some((k, v)) = line.split_once(':') {
            match (k.trim(), words(v).as_slice()) {
                ("Solvent Delay", [d, "min"]) => {
                    if let Some(d) = num(d) {
                        put("solvent_delay_min", json!(d));
                    }
                }
                ("Low Mass", [m]) => {
                    if let Some(m) = num(m) {
                        put("low_mass", json!(m));
                    }
                }
                ("High Mass", [m]) => {
                    if let Some(m) = num(m) {
                        put("high_mass", json!(m));
                    }
                }
                ("Acquistion Mode" | "Acquisition Mode", [m]) => {
                    put("ms_acquisition_mode", json!(m));
                }
                _ => {}
            }
        }
        if !line.is_empty()
            && !line.contains(':')
            && tokens.len() <= 5
            && !line.starts_with(char::is_numeric)
        {
            section = line.to_string();
        }
        i += 1;
    }
    let useful = out.keys().any(|k| k != "instrument_name");
    useful.then(|| {
        out.insert("file".into(), json!("acqmeth.txt"));
        Value::Object(out)
    })
}

#[cfg(test)]
mod tests {
    use super::parse_acqmeth;

    const GC: &str = "\r\n                INSTRUMENT CONTROL PARAMETERS:    GCMSD_1\n -----------------------------------------\n\n   C:\\MSDCHEM\\1\\METHODS\\PNNL_METABOLOMICS.M\n      Sun Feb 03 20:53:40 2013\n\nOven\n   Equilibration Time                         1 min\n   Oven Program                               On\n       60 \u{b0}C for 1 min\n       then 10 \u{b0}C/min to 325 \u{b0}C for 10 min\n   Run Time                                   37.5 min\n\nFront Injector\n   Injection Volume                           1 \u{b5}L\nFront SS Inlet He\n   Mode                                       Splitless\n   Heater                        On      250 \u{b0}C\nColumn #1\n   HP-5MS 5% Phenyl Methyl Silox: 866.55189\n   HP-5MS 5% Phenyl Methyl Silox\n   325 \u{b0}C: 30 m x 250 \u{b5}m x 0.25 \u{b5}m\n   In: Front SS Inlet He\nSolvent Delay     :  6.50 min\nLow Mass          :  50.0\nHigh Mass         :  600.0\n TUNE PARAMETERS for SN: US92032548\n";

    #[test]
    fn gc_method() {
        let m = parse_acqmeth(GC).unwrap();
        assert_eq!(m["instrument_name"], "GCMSD_1");
        assert_eq!(
            m["method_path"],
            "C:\\MSDCHEM\\1\\METHODS\\PNNL_METABOLOMICS.M"
        );
        assert_eq!(m["oven_program"][0]["temperature_c"], 60.0);
        assert_eq!(m["oven_program"][0]["hold_min"], 1.0);
        assert_eq!(m["oven_program"][1]["rate_c_per_min"], 10.0);
        assert_eq!(m["oven_program"][1]["temperature_c"], 325.0);
        assert_eq!(m["run_time_min"], 37.5);
        assert_eq!(m["injection_volume_ul"], 1.0);
        assert_eq!(m["inlet_mode"], "Splitless");
        assert_eq!(
            m["column"],
            "HP-5MS 5% Phenyl Methyl Silox, 30 m x 250 \u{b5}m x 0.25 \u{b5}m"
        );
        assert_eq!(m["solvent_delay_min"], 6.5);
        assert_eq!(m["low_mass"], 50.0);
        assert_eq!(m["high_mass"], 600.0);
        assert_eq!(m["ms_serial"], "US92032548");
    }

    #[test]
    fn unrelated_text() {
        assert!(parse_acqmeth("hello\nworld").is_none());
        assert!(parse_acqmeth("").is_none());
    }
}
