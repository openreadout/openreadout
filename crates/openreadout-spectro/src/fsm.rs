//! PerkinElmer Spotlight `.fsm` images: `PEPE`, a 40-byte description (`DataSet - 4D…`), then
//! blocks: 5100 the image geometry and spectral axis, 5104 the history and instrument records
//! (the `.sp` block tree), 5105 one spectrum per pixel.
//!
//! Layout and vocabulary: `docs/formats/perkinelmer-fsm.md`; provenance:
//! `docs/provenance/perkinelmer-fsm.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::common::{
    Facts, Le, MapSpec, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, num, read_at,
};
use crate::pesp::{
    history_records, instrument_record, instrument_words, json_tree, number_of, pe_time, tree,
    walk, x_axis, y_axis,
};

const FMT: &str = crate::FSM_FORMAT_ID;
/// Largest geometry or text block read (the spectra are read lazily).
const MAX_META: u64 = 16 << 20;

/// True when the 40-byte description after `PEPE` names a 4-D (image) data set.
pub(crate) fn is_image_description(head: &[u8]) -> bool {
    head.get(4..44)
        .is_some_and(|d| d.windows(2).any(|w| w == b"4D"))
}

/// The image geometry (block 5100).
#[derive(Debug, Clone)]
struct Geometry {
    name: String,
    step: [f64; 3],
    z_first: f64,
    z_last: f64,
    origin: [f64; 3],
    size: [u32; 3],
    labels: Vec<String>,
}

fn geometry(b: &[u8]) -> Option<Geometry> {
    let n = usize::from(b.u16_at(0)?);
    let name = crate::common::text_field(b.bytes_at(2, n)?);
    let at = 2 + n;
    let f = |k: usize| b.f64_at(at + 8 * k);
    let sizes = at + 80;
    let size = [
        u32::try_from(b.i32_at(sizes)?).ok()?,
        u32::try_from(b.i32_at(sizes + 4)?).ok()?,
        u32::try_from(b.i32_at(sizes + 8)?).ok()?,
    ];
    let mut labels = Vec::new();
    let mut p = sizes + 12;
    while labels.len() < 4 {
        let Some(len) = b.u16_at(p).map(usize::from) else {
            break;
        };
        let Some(t) = b.bytes_at(p + 2, len) else {
            break;
        };
        labels.push(crate::common::text_field(t));
        p += 2 + len;
    }
    Some(Geometry {
        name,
        step: [f(0)?, f(1)?, f(2)?],
        z_first: f(3)?,
        z_last: f(4)?,
        origin: [f(7)?, f(8)?, f(9)?],
        size,
        labels,
    })
}

/// One top-level block: id, body offset, body size.
type Block = (u16, u64, u64);

/// Parse a PerkinElmer `.fsm` image.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let head = read_at(f, path, 0, 44, file_len)?;
    if head.len() < 44 || head[..4] != crate::pesp::PESP_MAGIC[..] {
        return Err(Error::corrupt(
            FMT,
            "not a PerkinElmer file: it does not start with PEPE and a 40-byte description",
        ));
    }
    let description = crate::common::text_field(&head[4..44]);
    let mut parsed = Parsed::default();
    let blocks = block_list(f, path, file_len, &mut parsed)?;
    let meta = |id: u16| -> Result<Option<Vec<u8>>> {
        match blocks.iter().find(|b| b.0 == id) {
            Some(&(_, off, size)) if size <= MAX_META => {
                Ok(Some(read_at(f, path, off, size, file_len)?))
            }
            Some(_) => Err(Error::corrupt(
                FMT,
                format!("block {id} is implausibly large"),
            )),
            None => Ok(None),
        }
    };
    let g = meta(5100)?.as_deref().and_then(geometry).ok_or_else(|| {
        Error::corrupt(FMT, "the image geometry block (5100) is missing or short")
    })?;
    let [w, h, n] = g.size;
    if w == 0 || h == 0 || n == 0 {
        return Err(Error::corrupt(
            FMT,
            format!("image size {w} × {h} × {n} points"),
        ));
    }
    let spectra = spectrum_offsets(&blocks, &g, &mut parsed)?;
    let have = u32::try_from(spectra.len()).unwrap_or(u32::MAX);
    let first_spectrum = spectra.first().map(|o| o.saturating_sub(6));

    let mut extra = BTreeMap::new();
    let (history, tree_json) = match meta(5104)? {
        Some(b) => instrument_block(&b, &mut extra, &mut parsed),
        None => (Vec::new(), Value::Null),
    };
    parsed.facts = image_facts(&g, &history, &mut extra);

    let x_unit = g.labels.get(2).cloned().unwrap_or_default();
    let y_unit = g.labels.get(3).cloned().unwrap_or_default();
    let (xq, xu, dtype) = x_axis(&x_unit);
    let (yq, yu) = y_axis(&y_unit);
    if n > 1 {
        let step = (g.z_last - g.z_first) / f64::from(n - 1);
        if (step - g.step[2]).abs() > 1e-6 * g.step[2].abs().max(1.0) {
            parsed.findings.push(Finding::warning(
                "x_interval",
                format!(
                    "the stored step {} disagrees with {} → {} over {n} points",
                    g.step[2], g.z_first, g.z_last
                ),
            ));
        }
    }
    extra.insert("size_x".into(), json!(w));
    extra.insert("size_y".into(), json!(h));
    extra.insert("x_units_text".into(), json!(x_unit));
    extra.insert("y_units_text".into(), json!(y_unit));
    extra.insert(
        "stage_axis_labels".into(),
        json!([g.labels.first(), g.labels.get(1)]),
    );
    extra.insert(
        "origin_um".into(),
        json!([num(g.origin[0]), num(g.origin[1])]),
    );
    extra.insert("step_um".into(), json!([num(g.step[0]), num(g.step[1])]));
    parsed.sets.push(SpectrumSet {
        name: if g.name.is_empty() {
            format!("{yq} spectra")
        } else {
            g.name.clone()
        },
        x_quantity: xq,
        x_unit: xu.map(str::to_string),
        x: XValues::Regular {
            first: g.z_first,
            last: g.z_last,
        },
        y_name: yq.into(),
        y_unit: yu,
        points: u64::from(n),
        count: have,
        rows: Rows::Listed(spectra),
        stored: Stored::F32,
        scale: 1.0,
        data_type: dtype,
        extra,
    });
    parsed.tables.push(positions_table(&g, have));
    parsed.maps.push(image_map(&g, have));
    parsed.vendor = json!({
        "description": description,
        "geometry": {"name": g.name, "step": g.step.iter().map(|v| num(*v)).collect::<Vec<_>>(),
                     "z_first": num(g.z_first), "z_last": num(g.z_last),
                     "origin": g.origin.iter().map(|v| num(*v)).collect::<Vec<_>>(),
                     "size": g.size, "labels": g.labels},
        "history": history,
        "blocks_5104": tree_json,
        "block_counts": {
            "5100": blocks.iter().filter(|b| b.0 == 5100).count(),
            "5104": blocks.iter().filter(|b| b.0 == 5104).count(),
            "5105": blocks.iter().filter(|b| b.0 == 5105).count(),
            "other": blocks.iter().filter(|b| !matches!(b.0, 5100 | 5104 | 5105)).count(),
        },
    });
    for &(id, off, size) in blocks.iter().filter(|b| b.0 != 5105).take(64) {
        parsed.entries.push(LsEntry {
            kind: "block".into(),
            name: format!("block {id}"),
            offset: Some(off.saturating_sub(6)),
            size: Some(size + 6),
            image: None,
            details: json!({"id": id}),
        });
    }
    parsed.entries.push(LsEntry {
        kind: "spectra".into(),
        name: format!("{have} spectrum blocks (5105)"),
        offset: first_spectrum,
        size: Some(u64::from(have) * (u64::from(n) * 4 + 6)),
        image: Some(0),
        details: Value::Null,
    });
    parsed
        .notes
        .push(format!("PerkinElmer image file: {description}"));
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.y_quantity", Source::Inferred),
        ("traces[].extra.instrument", Source::PriorArt),
        ("traces[].extra.acquired_at", Source::Inferred),
        ("images[].physical_size", Source::PriorArt),
        ("tables[].columns", Source::Inferred),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// The top-level blocks after the description, up to the first that is cut off or invalid
/// (recorded as a finding).
fn block_list(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    parsed: &mut Parsed,
) -> Result<Vec<Block>> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut at = 44u64;
    while at < file_len {
        let h = read_at(f, path, at, 6, file_len)?;
        if h.len() < 6 {
            parsed.findings.push(Finding::error(
                "truncated",
                format!("a block header at {at} is cut off"),
            ));
            break;
        }
        let id = u16::from_le_bytes([h[0], h[1]]);
        let size = i32::from_le_bytes([h[2], h[3], h[4], h[5]]);
        let Ok(size) = u64::try_from(size) else {
            parsed.findings.push(Finding::error(
                "block_size",
                format!("block {id} at {at} has a negative size"),
            ));
            break;
        };
        let body = at + 6;
        if body.checked_add(size).is_none_or(|e| e > file_len) {
            parsed.findings.push(Finding::error(
                "truncated",
                format!("block {id} at {at} runs past the end of the file"),
            ));
            break;
        }
        blocks.push((id, body, size));
        at = body + size;
        if blocks.len() > 50_000_000 {
            return Err(Error::corrupt(FMT, "more blocks than any image holds"));
        }
    }
    Ok(blocks)
}

/// The body offsets of the spectrum blocks (5105), one per pixel in acquisition order.
fn spectrum_offsets(blocks: &[Block], g: &Geometry, parsed: &mut Parsed) -> Result<Vec<u64>> {
    let [w, h, n] = g.size;
    let spectra: Vec<u64> = blocks
        .iter()
        .filter(|b| b.0 == 5105)
        .map(|b| {
            if b.2 == u64::from(n) * 4 {
                b.1
            } else {
                u64::MAX
            }
        })
        .collect();
    if spectra.contains(&u64::MAX) {
        return Err(Error::corrupt(
            FMT,
            format!("a spectrum block (5105) is not {n} float32 values"),
        ));
    }
    if spectra.len() as u64 != u64::from(w) * u64::from(h) {
        parsed.findings.push(Finding::error(
            "spectrum_count",
            format!("{} spectrum blocks for a {w} × {h} image", spectra.len()),
        ));
    }
    Ok(spectra)
}

/// The instrument and history records of block 5104 (the `.sp` block tree): the instrument
/// settings go into `extra`; returns the history records and the tree as JSON.
fn instrument_block(
    b: &[u8],
    extra: &mut BTreeMap<String, Value>,
    parsed: &mut Parsed,
) -> (Vec<Value>, Value) {
    let mut bad = false;
    let nodes = tree(b, 0, b.len(), 0, 0, &mut bad);
    if bad {
        parsed
            .notes
            .push("the instrument block (5104) ends early".into());
    }
    let mut all = Vec::new();
    walk(&nodes, &mut all);
    let history = history_records(&all);
    instrument_record(&all, extra);
    if let Some(v) = all
        .iter()
        .find(|n| n.id == 35840)
        .copied()
        .and_then(number_of)
    {
        extra.insert("scans".into(), num(v));
    }
    (history, json_tree(&nodes))
}

/// Experiment facts: the oldest history record's date and user, the instrument record, the
/// image name and the pixel size.
fn image_facts(g: &Geometry, history: &[Value], extra: &mut BTreeMap<String, Value>) -> Facts {
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "PerkinElmer", "format");
    if let Some(hist) = history.last() {
        if let Some(t) = hist["date"].as_str().and_then(pe_time) {
            extra.insert("acquired_at".into(), json!(t));
            Facts::text(&mut facts.started_at, &t, "history record date (35700)");
        }
        if let Some(u) = hist["user"].as_str() {
            Facts::text(&mut facts.operator, u, "history record user (35698)");
        }
    }
    if let Some(v) = extra.get("instrument").and_then(Value::as_str) {
        Facts::text(&mut facts.model, v, "instrument record model (35837)");
    }
    if let Some(v) = extra.get("instrument_serial").and_then(Value::as_str) {
        Facts::text(&mut facts.serial, v, "instrument record serial (35838)");
    }
    if let Some(v) = extra.get("instrument_firmware").and_then(Value::as_str) {
        Facts::text(
            &mut facts.software_version,
            v,
            "instrument record firmware (35839)",
        );
    }
    if !g.name.is_empty() {
        Facts::text(&mut facts.sample_name, &g.name, "image name (block 5100)");
    }
    for (k, name, unit) in [
        ("resolution_cm1", "resolution", Some("cm⁻¹")),
        ("scans", "scans", None),
        ("laser_wavenumber_cm1", "laser_wavenumber", Some("cm⁻¹")),
    ] {
        if let Some(v) = extra.get(k).and_then(Value::as_f64) {
            facts.number(
                name,
                v,
                unit,
                &format!("instrument settings ({k})"),
                Source::Inferred,
            );
        }
    }
    instrument_words(extra, &mut facts);
    facts.number(
        "pixel_size",
        g.step[0].abs(),
        Some("µm"),
        "image geometry x step (block 5100)",
        Source::PriorArt,
    );
    facts
}

/// Pixel and stage positions of each spectrum: pixel (i, j) at origin + (i, j) · step, µm.
fn positions_table(g: &Geometry, have: u32) -> SpectrumTable {
    let w = u64::from(g.size[0]);
    let mut cols: Vec<(String, Option<String>, Vec<f64>)> = vec![
        ("x_px".into(), None, Vec::new()),
        ("y_px".into(), None, Vec::new()),
        ("x_um".into(), Some("µm".into()), Vec::new()),
        ("y_um".into(), Some("µm".into()), Vec::new()),
    ];
    for k in 0..u64::from(have) {
        let i = (k % w) as f64;
        let j = (k / w) as f64;
        cols[0].2.push(i);
        cols[1].2.push(j);
        cols[2].2.push(g.origin[0] + i * g.step[0]);
        cols[3].2.push(g.origin[1] + j * g.step[1]);
    }
    SpectrumTable {
        name: "positions".into(),
        trace: 0,
        columns: cols,
    }
}

/// The image: one pixel per spectrum, in acquisition order.
fn image_map(g: &Geometry, have: u32) -> MapSpec {
    let [w, h, _] = g.size;
    let mut extra = BTreeMap::new();
    extra.insert(
        "row_order".into(),
        json!("acquisition order: y_um increases by one step per row"),
    );
    MapSpec {
        name: if g.name.is_empty() {
            "image".into()
        } else {
            g.name.clone()
        },
        set: 0,
        width: w,
        height: h,
        pixels: (0..u64::from(w) * u64::from(h))
            .map(|k| u32::try_from(k).ok().filter(|k| *k < have))
            .collect(),
        pixel_um: (Some(g.step[0].abs()), Some(g.step[1].abs())),
        extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_block() {
        let mut b = vec![5u8, 0];
        b.extend_from_slice(b"Image");
        for v in [
            50.0f64, 50.0, -2.0, 4000.0, 750.0, 0.0, 0.0, 17480.0, 2749.0, 4000.0,
        ] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for v in [4i32, 4, 1626] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for t in ["Y - micrometers", "X - micrometers", "cm-1", "%T"] {
            b.extend_from_slice(&u16::try_from(t.len()).unwrap().to_le_bytes());
            b.extend_from_slice(t.as_bytes());
        }
        let g = geometry(&b).unwrap();
        assert_eq!(g.name, "Image");
        assert_eq!(g.size, [4, 4, 1626]);
        assert_eq!(g.labels[2], "cm-1");
        assert_eq!(g.labels[3], "%T");
        assert!((g.z_last - 750.0).abs() < 1e-12);
        for cut in 0..b.len() {
            let _ = geometry(&b[..cut]);
        }
        let mut head = b"PEPE".to_vec();
        head.extend_from_slice(b"DataSet - 4DConst3DInterval");
        head.resize(44, 0);
        assert!(is_image_description(&head));
        head[14] = b'2';
        assert!(!is_image_description(&head));
    }
}
