//! High-content screening plates (`oracle/hcs.py`): the plate layout, completeness, channel
//! names, geometry and every present plane against the index parsed by the Python standard
//! library, tifffile's pixels and Bio-Formats' reading (agreement counts reported).

use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{FileInfo, plate::plate_layout};
use serde_json::Value;

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

/// Compare one plate. `Ok(detail)` or `Err(problems)`.
pub fn check_hcs(ds: &mut dyn Dataset, info: &FileInfo, o: &Value) -> Result<String, String> {
    let mut problems: Vec<String> = Vec::new();
    let Some(plate) = plate_layout(ds, info) else {
        return Err("the reader reports no plate layout".into());
    };
    let op = &o["plate"];
    let fields = u(op, "fields").unwrap_or(0);
    if info.images.len() as u64 != fields {
        problems.push(format!(
            "{} images, the index has {fields} fields",
            info.images.len()
        ));
    }
    for (k, ours) in [
        ("wells_imaged", plate.wells.len() as u64),
        ("planes_expected", plate.planes_expected),
        ("planes_missing", plate.planes_missing),
        ("planes_absent", plate.planes_absent),
    ] {
        if let Some(want) = u(op, k)
            && want != ours
        {
            problems.push(format!("plate.{k} = {ours}, oracle {want}"));
        }
    }
    if let Some(r) = u(op, "rows")
        && r != u64::from(plate.rows)
    {
        problems.push(format!("rows {} != {r}", plate.rows));
    }
    if let Some(c) = u(op, "columns")
        && c != u64::from(plate.columns)
    {
        problems.push(format!("columns {} != {c}", plate.columns));
    }
    if let Some(id) = op.get("id").and_then(Value::as_str)
        && plate.id.as_deref() != Some(id)
    {
        problems.push(format!("plate id {:?} != {id:?}", plate.id));
    }
    for w in o["wells"].as_array().into_iter().flatten() {
        let name = w["well"].as_str().unwrap_or("");
        let want: Vec<u64> = w["images"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect();
        match plate.well(name) {
            Some(pw) => {
                let got: Vec<u64> = pw.images.iter().map(|&i| u64::from(i)).collect();
                if got != want {
                    problems.push(format!("well {name}: images {got:?} != {want:?}"));
                }
            }
            None => problems.push(format!("well {name} is not in the plate layout")),
        }
    }
    let want_names: Vec<&str> = o["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if let Some(im) = info.images.first() {
        let got: Vec<&str> = im
            .channels
            .iter()
            .map(|c| c.name.as_deref().unwrap_or(""))
            .collect();
        if !want_names.is_empty() && got != want_names {
            problems.push(format!("channel names {got:?} != {want_names:?}"));
        }
        // Bio-Formats' channel names and physical pixel size, when it names them (a second,
        // independent reading of the index; the index oracle alone once missed names kept
        // outside the Image records)
        let bf = &o["bioformats"];
        let bf_names: Vec<&str> = bf["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        // (Harmony and Columbus only: Bio-Formats names CellVoyager channels by acquisition action)
        if o["format"].as_str() == Some("opera-harmony")
            && bf_names.len() == got.len()
            && bf_names.iter().any(|n| !n.is_empty())
            && got != bf_names
        {
            problems.push(format!(
                "channel names {got:?} != Bio-Formats' {bf_names:?}"
            ));
        }
        if let Some(px) = bf["physical_size_um"].as_array()
            && let (Some(bx), Some(by)) = (
                px.first().and_then(Value::as_f64),
                px.get(1).and_then(Value::as_f64),
            )
        {
            let near = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() <= 1e-6 * b.abs());
            if !near(im.physical_size.x, bx) || !near(im.physical_size.y, by) {
                problems.push(format!(
                    "pixel size {:?} x {:?} != Bio-Formats' {bx} x {by}",
                    im.physical_size.x, im.physical_size.y
                ));
            }
        }
        for (k, ours) in [
            ("size_c", im.size_c),
            ("size_z", im.size_z),
            ("size_t", im.size_t),
        ] {
            if let Some(want) = u(o, k)
                && want != u64::from(ours)
            {
                problems.push(format!("{k} {ours} != {want}"));
            }
        }
        if let Some(sz) = o["size"].as_array()
            && sz.len() == 2
            && (Some(u64::from(im.size_x)), Some(u64::from(im.size_y)))
                != (sz[0].as_u64(), sz[1].as_u64())
        {
            problems.push(format!("size {}x{} != {:?}", im.size_x, im.size_y, sz));
        }
        if let Some(pt) = o["pixel_type"].as_str()
            && im.pixel_type.ome_name() != pt
        {
            problems.push(format!("pixel type {} != {pt}", im.pixel_type.ome_name()));
        }
        if let Some(px) = o["pixel_size_um"].as_array() {
            let want = (
                px.first().and_then(Value::as_f64),
                px.get(1).and_then(Value::as_f64),
            );
            let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * a.abs().max(b.abs()),
                (None, None) => true,
                _ => false,
            };
            if !close(im.physical_size.x, want.0) || !close(im.physical_size.y, want.1) {
                problems.push(format!(
                    "pixel size {:?} x {:?} != {want:?}",
                    im.physical_size.x, im.physical_size.y
                ));
            }
        }
    }
    // planes
    let (mut exact, mut missing_ok, mut absent_ok) = (0u64, 0u64, 0u64);
    for oim in o["images"].as_array().into_iter().flatten() {
        let Some(i) = u(oim, "index").and_then(|v| u32::try_from(v).ok()) else {
            continue;
        };
        let Some(im) = info.images.get(i as usize) else {
            problems.push(format!("image {i} does not exist"));
            continue;
        };
        if im.extra.get("well").and_then(Value::as_str) != oim["well"].as_str() {
            problems.push(format!(
                "image {i}: well {:?} != {:?}",
                im.extra.get("well"),
                oim["well"]
            ));
        }
        if im.extra.get("field").and_then(Value::as_u64) != u(oim, "field") {
            problems.push(format!(
                "image {i}: field {:?} != {:?}",
                im.extra.get("field"),
                oim["field"]
            ));
        }
        let absent = openreadout_core::plate::absent_planes(im);
        for pl in oim["planes"].as_array().into_iter().flatten() {
            let idx = PlaneIndex {
                c: u(pl, "c").unwrap_or(0) as u32,
                z: u(pl, "z").unwrap_or(0) as u32,
                t: u(pl, "t").unwrap_or(0) as u32,
            };
            let at = format!("image {i} c={} z={} t={}", idx.c, idx.z, idx.t);
            match pl["state"].as_str().unwrap_or("") {
                "present" => match ds.read_plane(i, idx) {
                    Ok(p) => {
                        let got = p.xxh3_hex();
                        if Some(got.as_str()) == pl["xxh3"].as_str() {
                            exact += 1;
                        } else {
                            problems.push(format!("{at}: hash {got} != {}", pl["xxh3"]));
                        }
                    }
                    Err(e) => problems.push(format!("{at}: read failed: {e}")),
                },
                "missing" => match ds.read_plane(i, idx) {
                    Err(e) if e.exit_code() == 5 => missing_ok += 1,
                    Err(e) => problems.push(format!(
                        "{at}: missing file gave {e} (exit {})",
                        e.exit_code()
                    )),
                    Ok(_) => problems.push(format!("{at}: missing file read without error")),
                },
                "not_acquired" | "not_recorded" => {
                    let blank = ds
                        .read_plane(i, idx)
                        .is_ok_and(|p| p.data.iter().all(|&b| b == 0));
                    if blank && absent.contains(&(idx.c, idx.z, idx.t)) {
                        absent_ok += 1;
                    } else {
                        problems.push(format!(
                            "{at}: absent plane not blank or not listed in extra.absent_planes"
                        ));
                    }
                }
                other => problems.push(format!("{at}: unknown oracle state {other}")),
            }
        }
    }
    let bf = &o["bioformats"];
    let bf_note = if bf.is_object() && bf.get("error").is_none() {
        let agree = u(bf, "planes_agree").unwrap_or(0);
        let cmp = u(bf, "planes_compared").unwrap_or(0);
        if u(bf, "planes_disagree").unwrap_or(0) > 0 {
            problems.push(format!(
                "Bio-Formats disagrees with tifffile on {} planes",
                u(bf, "planes_disagree").unwrap_or(0)
            ));
        }
        format!(
            "; Bio-Formats: {} series, {agree}/{cmp} planes equal to tifffile, absent planes {} blank / {} a repeat of the channel's acquired plane",
            u(bf, "series").unwrap_or(0),
            u(bf, "absent_planes_blank").unwrap_or(0),
            u(bf, "absent_planes_repeated").unwrap_or(0)
        )
    } else if let Some(e) = bf.get("error") {
        format!("; Bio-Formats: {e}")
    } else {
        String::new()
    };
    if problems.is_empty() {
        Ok(format!(
            "{} fields, {} wells, {exact} planes bit-exact vs tifffile, {missing_ok} missing files reported (exit 5), {absent_ok} absent planes blank{bf_note}",
            info.images.len(),
            plate.wells.len()
        ))
    } else {
        problems.truncate(12);
        Err(problems.join("; "))
    }
}
