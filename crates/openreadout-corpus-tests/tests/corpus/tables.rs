//! Plate-reader exports and tabular data sets (FCS).

use std::collections::BTreeMap;

use crate::covered;
use crate::oracle::{Oracle, OraclePlate, OraclePlateGroup};

/// Per detection mode: (row, col, value bits) triples and the distinct detection wavelengths.
pub(crate) type PlateGroup = (Vec<(u32, u32, u64)>, std::collections::BTreeSet<u64>);

/// Plate-reader exports against allotropy (oracle/plate.py): per detection mode, the measured
/// (not calculated) finite values of every table as (row, col, value) triples — count, distinct
/// wells, the xxh3-128 of the sorted triples, the detection wavelengths — and the instrument
/// header fields both sides report.
pub(crate) fn check_plate(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::model::FileInfo,
    oracle: &OraclePlate,
    counts_only: bool,
) -> Result<String, String> {
    // Per detection mode, the measured reads' values; and the calculated reads' values, which
    // are compared when the oracle separates them (`calculated_groups`).
    let mut groups: BTreeMap<String, PlateGroup> = BTreeMap::new();
    let mut calculated: BTreeMap<String, PlateGroup> = BTreeMap::new();
    for t in &info.tables {
        let reads = t
            .extra
            .get("reads")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let tab = ds
            .read_table(t.index, 0, t.row_count)
            .map_err(|e| format!("table {}: read failed: {e}", t.index))?;
        for i in 0..tab.columns[0].len() {
            let read = &reads[tab.columns[3][i] as usize - 1];
            let is_calc = read.get("calculated").and_then(serde_json::Value::as_bool) == Some(true);
            let v = tab.columns[6][i];
            let mode = read["mode"].as_str().unwrap_or("unknown").to_string();
            let g = if is_calc {
                calculated.entry(mode).or_default()
            } else {
                groups.entry(mode).or_default()
            };
            // A spectral scan's wavelengths are its rows' (non-numeric cells included, as the
            // oracle lists the scan's axis); otherwise the read's detection one.
            let scanned = read
                .get("settings")
                .and_then(|s| s.get("scanned_wavelength"))
                .is_some();
            let wl = if scanned {
                Some(tab.columns[4][i]).filter(|w| w.is_finite())
            } else {
                match read["mode"].as_str() {
                    Some("absorbance") => read.get("wavelength_nm"),
                    _ => read.get("emission_nm"),
                }
                .and_then(serde_json::Value::as_f64)
            };
            if let Some(w) = wl {
                g.1.insert(w.to_bits());
            }
            if !v.is_finite() {
                continue;
            }
            let v = if v == 0.0 { 0.0 } else { v };
            g.0.push((
                tab.columns[1][i] as u32 - 1,
                tab.columns[2][i] as u32 - 1,
                v.to_bits(),
            ));
        }
    }
    let mut problems = Vec::new();
    let mut summary = Vec::new();
    // Values are compared per detection mode: the reads' modes are checked with them.
    covered("tables[].extra.reads[].mode");
    if oracle.groups.iter().any(|g| !g.wavelengths.is_empty()) {
        covered("tables[].extra.reads[].wavelength_nm");
    }
    compare_plate_groups(
        &groups,
        &oracle.groups,
        counts_only,
        "",
        &mut problems,
        &mut summary,
    );
    if !oracle.calculated_groups.is_empty() {
        compare_plate_groups(
            &calculated,
            &oracle.calculated_groups,
            counts_only,
            "calculated ",
            &mut problems,
            &mut summary,
        );
    }
    let first = info.tables.first().map(|t| &t.extra);
    let ours = |k: &str| -> Option<String> {
        let e = first?;
        let v = match k {
            "model" | "serial_number" => e.get("instrument")?.get(k)?,
            _ => e.get(k)?,
        };
        v.as_str().map(str::to_string)
    };
    let mut header_ok = 0;
    for (k, v) in &oracle.header {
        if k == "model" && ours(k).is_some() {
            covered("experiment.instrument.model");
        }
        match ours(k) {
            Some(o) if &o == v => header_ok += 1,
            Some(o) => problems.push(format!("{k} {o:?} != oracle {v:?}")),
            None => {} // the oracle states a value the file does not hold (allotropy defaults)
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{} tables; {}; {header_ok} header fields match",
            info.tables.len(),
            summary.join(", ")
        ))
    } else {
        Err(problems.join("; "))
    }
}

/// Compare our per-mode values with the oracle's: count, distinct wells, xxh3-128 of the sorted
/// (row, col, value) triples, and the wavelengths.
pub(crate) fn compare_plate_groups(
    ours_by_mode: &BTreeMap<String, PlateGroup>,
    oracle_groups: &[OraclePlateGroup],
    counts_only: bool,
    what: &str,
    problems: &mut Vec<String>,
    summary: &mut Vec<String>,
) {
    let modes: std::collections::BTreeSet<&str> = oracle_groups
        .iter()
        .map(|g| g.mode.as_str())
        .chain(ours_by_mode.keys().map(String::as_str))
        .collect();
    for mode in modes {
        let (mut triples, wls) = ours_by_mode.get(mode).cloned().unwrap_or_default();
        let Some(o) = oracle_groups.iter().find(|g| g.mode == mode) else {
            problems.push(format!(
                "{what}{mode}: {} values only in ours",
                triples.len()
            ));
            continue;
        };
        triples.sort_unstable();
        let mut bytes = Vec::with_capacity(triples.len() * 16);
        for (r, c, v) in &triples {
            bytes.extend_from_slice(&r.to_le_bytes());
            bytes.extend_from_slice(&c.to_le_bytes());
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let hash = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
        let mut wells: Vec<(u32, u32)> = triples.iter().map(|t| (t.0, t.1)).collect();
        wells.sort_unstable();
        wells.dedup();
        if triples.len() != o.values || wells.len() != o.wells {
            problems.push(format!(
                "{what}{mode}: {} values in {} wells != oracle {} in {}",
                triples.len(),
                wells.len(),
                o.values,
                o.wells
            ));
        } else if hash != o.value_xxh3 && !counts_only {
            problems.push(format!(
                "{what}{mode}: value hash {hash} != oracle {}",
                o.value_xxh3
            ));
        }
        let ours_wl: Vec<f64> = wls.iter().map(|b| f64::from_bits(*b)).collect();
        if !o.wavelengths.is_empty() && !ours_wl.is_empty() && ours_wl != o.wavelengths {
            problems.push(format!(
                "{what}{mode}: wavelengths {ours_wl:?} != oracle {:?}",
                o.wavelengths
            ));
        }
        summary.push(format!(
            "{what}{mode} {} values/{} wells{}",
            o.values,
            o.wells,
            if counts_only { " (counts only)" } else { "" }
        ));
    }
}

/// Compare tables: count, row count, column names, dtypes, and the xxh3-128 of the whole event
/// matrix as column-major little-endian f64 (the definition in oracle/gen.py).
pub(crate) fn check_tables(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::model::FileInfo,
    oracle: &Oracle,
    problems: &mut Vec<String>,
    ok_tables: &mut usize,
    unhashed: &mut usize,
) {
    if info.tables.len() != oracle.tables.len() {
        problems.push(format!(
            "table count {} != oracle {}",
            info.tables.len(),
            oracle.tables.len()
        ));
    }
    for o in &oracle.tables {
        let Some(t) = info.tables.iter().find(|t| t.index == o.index) else {
            problems.push(format!("table {} missing", o.index));
            continue;
        };
        if t.row_count != o.event_count {
            problems.push(format!(
                "table {}: rows {} != oracle {}",
                o.index, t.row_count, o.event_count
            ));
        }
        let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        if !o.parameter_names.is_empty()
            && names
                != o.parameter_names
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
        {
            problems.push(format!(
                "table {}: columns {names:?} != oracle {:?}",
                o.index, o.parameter_names
            ));
        }
        let dtypes: Vec<&str> = t.columns.iter().map(|c| c.dtype.as_str()).collect();
        if !o.dtypes.is_empty() && dtypes != o.dtypes.iter().map(String::as_str).collect::<Vec<_>>()
        {
            problems.push(format!(
                "table {}: dtypes {dtypes:?} != oracle {:?}",
                o.index, o.dtypes
            ));
        }
        if !o.sorted_row_hashes.is_empty() {
            match ds.read_table(o.index, 0, t.row_count) {
                Ok(tab) => {
                    let mut bad = 0;
                    for sr in &o.sorted_row_hashes {
                        let idx: Option<Vec<usize>> = sr
                            .columns
                            .iter()
                            .map(|n| t.columns.iter().position(|c| &c.name == n))
                            .collect();
                        let Some(idx) = idx else {
                            problems.push(format!(
                                "table {}: missing one of the columns {:?}",
                                o.index, sr.columns
                            ));
                            bad += 1;
                            continue;
                        };
                        let n = tab.columns.first().map_or(0, Vec::len);
                        if n as u64 != sr.count {
                            problems.push(format!(
                                "table {} rows over {:?}: {n} != oracle {}",
                                o.index, sr.columns, sr.count
                            ));
                            bad += 1;
                            continue;
                        }
                        let mut rows: Vec<Vec<f64>> = (0..n)
                            .map(|r| idx.iter().map(|&c| tab.columns[c][r]).collect())
                            .collect();
                        rows.sort_by(|a, b| {
                            a.iter()
                                .zip(b)
                                .map(|(x, y)| x.total_cmp(y))
                                .find(|o| o.is_ne())
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let mut bytes = Vec::with_capacity(n * idx.len() * 8);
                        for row in &rows {
                            for v in row {
                                bytes.extend_from_slice(&v.to_le_bytes());
                            }
                        }
                        let got = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                        if got != sr.xxh3 {
                            bad += 1;
                            problems.push(format!(
                                "table {} sorted rows {:?}: hash {got} != oracle {}",
                                o.index, sr.columns, sr.xxh3
                            ));
                        }
                    }
                    if bad == 0 {
                        *ok_tables += 1;
                    }
                }
                Err(e) => problems.push(format!("table {}: read failed: {e}", o.index)),
            }
            continue;
        }
        if !o.sorted_column_hashes.is_empty() {
            match ds.read_table(o.index, 0, t.row_count) {
                Ok(tab) => {
                    let mut bad = 0;
                    let col = |name: &str| t.columns.iter().position(|c| c.name == name);
                    for sc in &o.sorted_column_hashes {
                        let Some(ci) = col(&sc.column) else {
                            problems.push(format!("table {}: no column {}", o.index, sc.column));
                            bad += 1;
                            continue;
                        };
                        let filter = sc.where_column.as_deref().and_then(col);
                        let mut vals: Vec<f64> = (0..tab.columns[ci].len())
                            .filter(|&r| {
                                filter.is_none_or(|w| Some(tab.columns[w][r]) == sc.where_value)
                            })
                            .map(|r| tab.columns[ci][r])
                            .collect();
                        if let Some(n) = sc.count
                            && vals.len() as u64 != n
                        {
                            problems.push(format!(
                                "table {} column {}: {} rows != oracle {n}",
                                o.index,
                                sc.column,
                                vals.len()
                            ));
                            bad += 1;
                            continue;
                        }
                        vals.sort_by(f64::total_cmp);
                        let mut bytes = Vec::with_capacity(vals.len() * 8);
                        for v in &vals {
                            bytes.extend_from_slice(&v.to_le_bytes());
                        }
                        let got = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                        if got != sc.xxh3 {
                            bad += 1;
                            problems.push(format!(
                                "table {} sorted column {}: hash {got} != oracle {}",
                                o.index, sc.column, sc.xxh3
                            ));
                        }
                    }
                    if bad == 0 {
                        *ok_tables += 1;
                    }
                }
                Err(e) => problems.push(format!("table {}: read failed: {e}", o.index)),
            }
            continue;
        }
        if !o.column_hashes.is_empty() {
            match ds.read_table(o.index, 0, t.row_count) {
                Ok(tab) => {
                    let mut bad = 0;
                    for (name, want) in &o.column_hashes {
                        let Some(ci) = t.columns.iter().position(|c| &c.name == name) else {
                            problems.push(format!("table {}: no column {name}", o.index));
                            bad += 1;
                            continue;
                        };
                        let mut bytes = Vec::with_capacity(tab.columns[ci].len() * 8);
                        for v in &tab.columns[ci] {
                            bytes.extend_from_slice(&v.to_le_bytes());
                        }
                        let got = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                        if &got != want {
                            bad += 1;
                            if bad <= 3 {
                                problems.push(format!(
                                    "table {} column {name}: hash {got} != oracle {want}",
                                    o.index
                                ));
                            }
                        }
                    }
                    if bad == 0 {
                        *ok_tables += 1;
                    }
                }
                Err(e) => problems.push(format!("table {}: read failed: {e}", o.index)),
            }
            continue;
        }
        let Some(want) = &o.xxh3 else {
            *unhashed += 1;
            continue;
        };
        match ds.read_table(o.index, 0, t.row_count) {
            Ok(tab) => {
                let mut bytes =
                    Vec::with_capacity(tab.columns.iter().map(Vec::len).sum::<usize>() * 8);
                for col in &tab.columns {
                    for v in col {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                }
                let got = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                if &got == want {
                    *ok_tables += 1;
                } else {
                    problems.push(format!(
                        "table {}: event hash {got} != oracle {want}",
                        o.index
                    ));
                }
            }
            Err(e) => problems.push(format!("table {}: read failed: {e}", o.index)),
        }
    }
}
