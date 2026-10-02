//! SoftMax Pro 5 binary documents (`.pda`): a big-endian stream of objects whose class names
//! start with `CS` (docs/formats/plate-readers.md, "SoftMax Pro documents"; provenance
//! 2026-09-25).
//!
//! Decoded: plate sections (`CSPlateSection` → `CSPlateData` settings, `CSPlateDescriptor`
//! per-read temperatures, read count and time stamp, one `CSSite` per well with its values)
//! and the template (groups, samples and their wells) as the plate layout. Only the layouts
//! the corpus validates are decoded; other plate sections are listed and refused.

use std::collections::BTreeMap;

use openreadout_core::bytes::latin1;
use openreadout_core::model::Finding;
use openreadout_core::time::civil_from_days;
use serde_json::{Value, json};

use super::cursor::{Cursor, find};
use crate::grid::well_name;
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType};
use crate::sheet::Container;

const BLOCKS: &[u8] = b"##BLOCKS=";
const PLATE_SECTION: &[u8] = b"\x0eCSPlateSection";
const PLATE_DATA: &[u8] = b"\x0bCSPlateData";
const DESCRIPTOR: &[u8] = b"\x11CSPlateDescriptor";
const SITE: &[u8] = b"\x06CSSite";
const MORPH_TABLE: &[u8] = b"\x11CSMorphPlateTable";
const CUVETTE_SECTION: &[u8] = b"\x10CSCuvetteSection";
/// Seconds from 1904-01-01 (the time stamps' epoch) to 1970-01-01.
const EPOCH_1904: i64 = 2_082_844_800;
/// Largest kinetic run read (reads × wells values stay far below the 256 MiB file cap).
const MAX_READS: u32 = 1_000_000;

/// `SoftMax Pro\0` or two bytes, a version text and `##BLOCKS=` within the first 64 bytes,
/// with NUL bytes before it (the text export starts with `##BLOCKS=` itself).
pub(crate) fn sniff(head: &[u8]) -> bool {
    let Some(p) = find(head, BLOCKS, 0, 64) else {
        return false;
    };
    let pre = &head[..p];
    pre.contains(&0) && version_text(pre).is_some()
}

/// `5.42.1.0` from ` 5.42.1.0\0\0`.
fn version_text(pre: &[u8]) -> Option<String> {
    pre.split(|&b| b == 0)
        .map(|t| {
            String::from_utf8_lossy(t)
                .trim_matches(|c: char| c.is_whitespace() || c.is_control())
                .to_string()
        })
        .find(|t| {
            t.len() >= 3
                && t.chars().next().is_some_and(|c| c.is_ascii_digit())
                && t.contains('.')
                && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

/// A plate section's fixed settings (`CSPlateData`).
#[derive(Debug, Clone)]
struct PlateData {
    id: u32,
    read_type: u16,
    read_mode: u16,
    columns: u16,
    reads: u32,
    wavelengths: u32,
    wavelength: u32,
    run_time_s: f64,
    interval_s: f64,
    temperature_control: bool,
    temperature_set_c: f64,
}

fn plate_data(c: &mut Cursor) -> Option<PlateData> {
    let id = c.u32_be()?;
    let read_type = c.u16_be()?;
    let read_mode = c.u16_be()?;
    let columns = c.u16_be()?;
    let reads = c.u32_be()?;
    let wavelengths = c.u32_be()?;
    let wavelength = c.u32_be()?;
    c.skip(5)?;
    let run_time_s = c.f64_be()?;
    let interval_s = c.f64_be()?;
    let temperature_control = c.u8()? != 0;
    let temperature_set_c = c.f64_be()?;
    Some(PlateData {
        id,
        read_type,
        read_mode,
        columns,
        reads,
        wavelengths,
        wavelength,
        run_time_s,
        interval_s,
        temperature_control,
        temperature_set_c,
    })
}

/// One template well: sample, group and the section it belongs to.
#[derive(Debug, Clone)]
struct TemplateWell {
    group: String,
    sample: String,
    row: u16,
    col: u16,
    section_id: u32,
    section: String,
    cuvette: bool,
}

/// The template's groups (name, unit, column) and wells; `None` when it does not parse.
fn template(data: &[u8]) -> Option<(Vec<Value>, Vec<TemplateWell>)> {
    let start = find(data, b"\x13CSExperimentSection", 0, data.len())?;
    let t = find(data, b"\x01Template\0", start, start.checked_add(128)?)?;
    let mut c = Cursor::new(data, t + 10);
    let ngroups = c.u32_be()?;
    if ngroups > 10_000 {
        return None;
    }
    let mut groups = Vec::new();
    let mut wells = Vec::new();
    for _ in 0..ngroups {
        c.expect(b"\x0bCSTmplGroup")?;
        let group = c.cstr(255)?;
        c.skip(8)?;
        let unit = c.cstr(255)?;
        c.skip(4)?;
        let column = c.cstr(255)?;
        c.skip(7 + 14)?;
        let nsamples = c.u32_be()?;
        if nsamples > 100_000 {
            return None;
        }
        let mut samples = Vec::new();
        for _ in 0..nsamples {
            c.expect(b"\x0cCSTmplSample")?;
            let sample = c.cstr(255)?;
            c.skip(8 + 16)?;
            let nwells = c.u32_be()?;
            if nwells > 1_000_000 {
                return None;
            }
            let mut names = Vec::new();
            for _ in 0..nwells {
                let len = c.u8()?;
                let class = c.take(usize::from(len))?;
                let cuvette = match class {
                    b"CSWell" => false,
                    b"CSCuvetteWell" => true,
                    _ => return None,
                };
                let name = c.cstr(16)?;
                let row = c.u16_be()?;
                let col = c.u16_be()?;
                let section_id = c.u32_be()?;
                c.skip(2)?;
                let section = c.cstr(255)?;
                c.skip(4)?;
                names.push(name);
                wells.push(TemplateWell {
                    group: group.clone(),
                    sample: sample.clone(),
                    row,
                    col,
                    section_id,
                    section,
                    cuvette,
                });
            }
            samples.push(json!({"sample": sample, "wells": names}));
        }
        let mut g = json!({"group": group, "samples": samples});
        if !unit.is_empty() {
            g["unit"] = json!(unit);
        }
        if !column.is_empty() {
            g["column"] = json!(column);
        }
        groups.push(g);
    }
    // the template ends with a u32 and the next section's class name (`CSAnalysisSection…`)
    c.skip(5)?;
    c.at(b"CS").then_some((groups, wells))
}

/// Seconds since 1904-01-01 (local time) as ISO-8601 without a zone.
fn iso_1904(secs: u32) -> Option<String> {
    let unix = i64::from(secs) - EPOCH_1904;
    let days = unix.div_euclid(86_400);
    let tod = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (1900..3000).contains(&y).then(|| {
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
            tod / 3600,
            tod % 3600 / 60,
            tod % 60
        )
    })
}

/// The instrument string written just before `CSMorphPlateTable`
/// (`SPECTRAmax M5 ROM v2.1.35 20May09`).
fn instrument(data: &[u8], from: usize, to: usize) -> Option<String> {
    let m = find(data, MORPH_TABLE, from, to)?;
    let end = m.checked_sub(1)?;
    if data.get(end) != Some(&0) {
        return None;
    }
    let start = data[..end]
        .iter()
        .rposition(|&b| b == 0)
        .map_or(0, |p| p + 1);
    let s = latin1(&data[start..end]);
    (s.len() >= 4 && s.chars().all(|c| !c.is_control())).then_some(s)
}

pub(crate) fn parse(data: &[u8]) -> Option<Export> {
    let head_end = find(data, BLOCKS, 0, 64)?;
    let mut ex = Export::new(
        Kind::SoftMaxPro,
        Container::Binary {
            kind: "softmax-pro-5-document",
        },
    );
    ex.put("document", "SoftMax Pro 5 binary document");
    if let Some(v) = version_text(&data[..head_end]) {
        ex.software_version = Some(v.clone());
        ex.put("SoftMax Pro version", v);
    }
    let blocks_line: String = data[head_end..data.len().min(head_end + 32)]
        .iter()
        .take_while(|&&b| b != b'\r' && b != 0)
        .map(|&b| char::from(b))
        .collect();
    ex.put(
        "##BLOCKS",
        blocks_line.trim_start_matches("##BLOCKS=").trim(),
    );
    let layout = template(data);
    if let Some((groups, _)) = &layout {
        ex.sections.insert("template".into(), json!(groups));
    } else if find(data, b"\x13CSExperimentSection", 0, data.len()).is_some() {
        ex.findings.push(Finding::info(
            "template_not_read",
            "the document's template (groups, samples, wells) has a layout that was not recognised; plate values are unaffected, no plate layout is reported",
        ));
    }
    let mut refused = Vec::new();
    let mut pos = 0;
    while let Some(at) = find(data, PLATE_SECTION, pos, data.len()) {
        pos = at + PLATE_SECTION.len();
        let mut c = Cursor::new(data, pos);
        let Some(name) = c.cstr(64) else { continue };
        let Some(next) = c.u32_be() else { continue };
        if !c.at(PLATE_DATA) {
            continue;
        }
        let end = usize::try_from(next)
            .ok()
            .filter(|&n| n > c.pos && n <= data.len())
            .unwrap_or(data.len());
        c.pos += PLATE_DATA.len();
        let Some(pd) = plate_data(&mut c) else {
            refused.push(format!("{name}: settings cut short"));
            continue;
        };
        match plate(data, &name, &pd, c.pos, end, layout.as_ref().map(|l| &l.1)) {
            Ok(Some(mut b)) => {
                if let Some(s) = instrument(data, c.pos, end) {
                    if let Some((model, rom)) = s.split_once(" ROM ") {
                        ex.model.get_or_insert_with(|| model.trim().to_string());
                        b.extra
                            .insert("firmware".into(), json!(format!("ROM {}", rom.trim())));
                    } else {
                        ex.model.get_or_insert(s);
                    }
                }
                ex.blocks.push(b);
            }
            Ok(None) => ex.notes.push(format!(
                "plate section {name:?} holds no data (the plate was not read)"
            )),
            Err(why) => refused.push(why),
        }
        pos = end.max(pos);
    }
    if let Some(at) = find(data, CUVETTE_SECTION, 0, data.len())
        && find(data, b"\x0dCSCuvetteSite", at, data.len()).is_some()
    {
        refused.push("cuvette-set data (CSCuvetteSection): not decoded".into());
    }
    if !refused.is_empty() {
        ex.sections.insert("refused_plates".into(), json!(refused));
        ex.findings.push(Finding::warning(
            "plate_not_decoded",
            format!(
                "{} data section(s) are not decoded because their layout has not been validated: {}; export them as text from SoftMax Pro",
                refused.len(),
                refused.join("; ")
            ),
        ));
    }
    if let Some(t) = ex.blocks.iter().find_map(|b| b.started_at.clone()) {
        ex.acquired_at = Some(t.clone());
        ex.acquired_raw = Some(t);
    }
    Some(ex)
}

/// A plate section as a block; `Ok(None)` when it holds no data, `Err` when refused.
fn plate(
    data: &[u8],
    name: &str,
    pd: &PlateData,
    from: usize,
    end: usize,
    layout: Option<&Vec<TemplateWell>>,
) -> Result<Option<Block>, String> {
    let Some(desc) = find(data, DESCRIPTOR, from, end) else {
        return Ok(None);
    };
    let read_type = match pd.read_type {
        0 => ReadType::Endpoint,
        1 => ReadType::Kinetic,
        t => {
            return Err(format!(
                "{name}: read type {t} (endpoint and kinetic are validated)"
            ));
        }
    };
    if pd.read_mode != 1 {
        return Err(format!(
            "{name}: read mode {} (only absorbance, mode 1, is validated)",
            pd.read_mode
        ));
    }
    if pd.wavelengths != 1 {
        return Err(format!(
            "{name}: {} wavelengths (one is validated)",
            pd.wavelengths
        ));
    }
    if pd.columns != 12 {
        return Err(format!(
            "{name}: {} columns (96-well plates are validated)",
            pd.columns
        ));
    }
    if pd.reads == 0 || pd.reads > MAX_READS {
        return Err(format!("{name}: {} reads", pd.reads));
    }
    let mut cur = Cursor::new(data, desc + DESCRIPTOR.len());
    let cut = || format!("{name}: plate descriptor cut short");
    let _flag = cur.u8().ok_or_else(cut)?;
    if cur.u32_be().ok_or_else(cut)? != pd.reads {
        return Err(format!(
            "{name}: descriptor read count differs from the settings"
        ));
    }
    let mut temps = Vec::with_capacity(pd.reads as usize);
    for _ in 0..pd.reads {
        cur.skip(4).ok_or_else(cut)?;
        temps.push(f64::from(cur.f32_be().ok_or_else(cut)?));
    }
    let _one = cur.u32_be().ok_or_else(cut)?;
    if cur.u32_be().ok_or_else(cut)? != pd.reads {
        return Err(format!(
            "{name}: descriptor read count differs from the settings"
        ));
    }
    let stamp = cur.u32_be().ok_or_else(cut)?;
    cur.skip(4).ok_or_else(cut)?;
    let done = cur.u32_be().ok_or_else(cut)?;
    cur.skip(3).ok_or_else(cut)?;
    let id = cur.u32_be().ok_or_else(cut)?;
    if id != pd.id || done > pd.reads {
        return Err(format!(
            "{name}: descriptor does not match its section (id {id} vs {}, {done} of {} reads)",
            pd.id, pd.reads
        ));
    }
    let n_values = pd.reads as usize + 1;
    let mut sites: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    while cur.at(SITE) && cur.pos < end {
        cur.pos += SITE.len();
        let hdr = (cur.u32_be(), cur.u32_be(), cur.u32_be(), cur.u32_be());
        let (Some(nwl), Some(nr), Some(idx), Some(nb)) = hdr else {
            return Err(format!("{name}: well record cut short"));
        };
        if nwl != 1 || nr != pd.reads || nb as usize != n_values * 8 || !(1..=96).contains(&idx) {
            return Err(format!(
                "{name}: well record {idx} has an unvalidated layout ({nwl} wavelengths, {nr} reads, {nb} bytes)"
            ));
        }
        let raw = cur
            .take(nb as usize)
            .ok_or_else(|| format!("{name}: well values cut short"))?;
        cur.skip(nb as usize)
            .ok_or_else(|| format!("{name}: well record cut short"))?;
        let vals: Vec<f64> = raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| f64::from_be_bytes(*b))
            .collect();
        if sites.insert(idx, vals).is_some() {
            return Err(format!("{name}: well {idx} recorded twice"));
        }
    }
    if sites.len() != 96 {
        return Err(format!(
            "{name}: {} well records (96 are validated)",
            sites.len()
        ));
    }
    let mut b = Block::new(name, name);
    b.rows = 8;
    b.cols = 12;
    b.declared_wells = Some(96);
    b.read_type = Some(read_type);
    let mut ch = Channel::new(format!("Absorbance {}", pd.wavelength), Mode::Absorbance);
    ch.wavelength_nm = Some(f64::from(pd.wavelength));
    ch.unit = Some("OD".into());
    let chi = b.channel(ch);
    let kinetic = read_type == ReadType::Kinetic;
    for (idx, vals) in &sites {
        let i = idx - 1;
        let (r, col) = (i / 12, i % 12);
        for (k, v) in vals.iter().take(done as usize).enumerate() {
            let t = kinetic.then_some(k as f64 * pd.interval_s);
            let text = (!v.is_finite()).then(|| "no value".to_string());
            b.push_value(r, col, chi, t, *v, text);
        }
    }
    let temps: Vec<f64> = temps
        .into_iter()
        .take(done as usize)
        .map(|t| (t * 100.0).round() / 100.0)
        .collect();
    b.temperature_c = temps.first().copied();
    if kinetic {
        b.extra.insert("temperatures_c".into(), json!(temps));
        b.extra.insert("kinetic_points".into(), json!(pd.reads));
        b.extra.insert("reads_completed".into(), json!(done));
        b.extra
            .insert("read_interval_s".into(), json!(pd.interval_s));
        b.extra.insert("run_time_s".into(), json!(pd.run_time_s));
        b.extra.insert(
            "time_basis".into(),
            json!("read index × read interval (the document stores no per-read time)"),
        );
        if done < pd.reads {
            b.findings.push(Finding::info(
                "run_stopped_early",
                format!(
                    "{name}: {done} of {} planned kinetic reads were made; the rest were never measured and are left out",
                    pd.reads
                ),
            ));
        }
    }
    if pd.temperature_control {
        b.extra.insert(
            "temperature_set_point_c".into(),
            json!(pd.temperature_set_c),
        );
    }
    b.started_at = iso_1904(stamp);
    b.extra.insert("section_id".into(), json!(pd.id));
    if let Some(wells) = layout {
        let mut sample = serde_json::Map::new();
        let mut group = serde_json::Map::new();
        let mut role = serde_json::Map::new();
        for w in wells
            .iter()
            .filter(|w| !w.cuvette && (w.section_id == pd.id || w.section == name))
        {
            if w.row == 0 || w.col == 0 || w.row > 8 || w.col > 12 {
                continue;
            }
            let well = well_name(u32::from(w.row) - 1, u32::from(w.col) - 1);
            if !w.sample.is_empty() {
                sample.insert(well.clone(), json!(w.sample));
            }
            group.insert(well.clone(), json!(w.group));
            if w.group == "Blank" {
                role.insert(well, json!("blank"));
            }
        }
        let mut maps = serde_json::Map::new();
        if !sample.is_empty() {
            maps.insert("Sample".into(), Value::Object(sample));
        }
        if !group.is_empty() {
            maps.insert("SoftMax group".into(), Value::Object(group));
        }
        if !role.is_empty() {
            maps.insert("Role".into(), Value::Object(role));
            b.findings.push(Finding::info(
                "plate_blank_group",
                format!(
                    "{name}: the template has a `Blank` group; SoftMax Pro subtracts its mean from every well in its reduced and exported values, the values here are raw"
                ),
            ));
        }
        if !maps.is_empty() {
            b.extra.insert("layout".into(), Value::Object(maps));
        }
    }
    Ok(Some(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_epoch() {
        let mut h = b"SoftMax Pro\0\x04 5.42.1.0\0\0##BLOCKS= 4          \r".to_vec();
        h.resize(64, 0);
        assert!(sniff(&h));
        assert!(!sniff(b"##BLOCKS= 1\r\nPlate:\t"));
        assert!(sniff(b"\0\x06 5.4.12.1.0\0\0##BLOCKS= 1   "));
        assert_eq!(
            iso_1904(3_736_153_558).as_deref(),
            Some("2022-05-23T12:25:58")
        );
        assert_eq!(
            iso_1904(3_420_692_753).as_deref(),
            Some("2012-05-24T08:25:53")
        );
    }

    /// A one-read, 96-well absorbance document built from the observed layout.
    fn document(sites: usize) -> Vec<u8> {
        let mut d = b"\0\x06 5.4.12.1.0\0\0##BLOCKS= 1          \r\0\0\0\0\0\0\x07\xd0".to_vec();
        d.resize(2000, 0);
        d.extend_from_slice(PLATE_SECTION);
        d.extend_from_slice(b"Plate#1\0");
        let next_at = d.len();
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(PLATE_DATA);
        d.extend_from_slice(&700u32.to_be_bytes());
        d.extend_from_slice(&[0, 0, 0, 1, 0, 12]);
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&595u32.to_be_bytes());
        d.extend_from_slice(&[0; 5]);
        d.extend_from_slice(&300f64.to_be_bytes());
        d.extend_from_slice(&20f64.to_be_bytes());
        d.push(0);
        d.extend_from_slice(&37f64.to_be_bytes());
        d.extend_from_slice(DESCRIPTOR);
        d.push(1);
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&24.2f32.to_be_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&3_736_153_558u32.to_be_bytes());
        d.extend_from_slice(&[0, 1, 0, 12]);
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&[0, 1, 1]);
        d.extend_from_slice(&700u32.to_be_bytes());
        for i in 1..=sites as u32 {
            d.extend_from_slice(SITE);
            for v in [1, 1, i, 16] {
                d.extend_from_slice(&v.to_be_bytes());
            }
            d.extend_from_slice(&(f64::from(i) / 100.0).to_be_bytes());
            d.extend_from_slice(&[0; 8 + 16]);
        }
        let n = d.len() as u32;
        d[next_at..next_at + 4].copy_from_slice(&n.to_be_bytes());
        d
    }

    #[test]
    fn endpoint_plate() {
        let d = document(96);
        let ex = parse(&d).unwrap();
        assert!(ex.findings.is_empty(), "{:?}", ex.findings);
        let b = &ex.blocks[0];
        assert_eq!(b.obs.len(), 96);
        assert_eq!(b.obs[13].value.to_bits(), 0.14f64.to_bits());
        assert_eq!((b.obs[13].row, b.obs[13].col), (1, 1));
        assert_eq!(b.temperature_c, Some(24.2));
        assert_eq!(b.started_at.as_deref(), Some("2022-05-23T12:25:58"));
        assert_eq!(ex.software_version.as_deref(), Some("5.4.12.1.0"));
    }

    #[test]
    fn template_groups_samples_wells() {
        let mut d = b"\x13CSExperimentSectionExperiment#1\0\0\0\0\0".to_vec();
        d.extend_from_slice(&[0, 0, 1, 0, 0]);
        d.extend_from_slice(&[0xff; 15]);
        d.extend_from_slice(b"\x01Template\0");
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(b"\x0bCSTmplGroupBlank\0");
        d.extend_from_slice(&1f64.to_be_bytes());
        d.extend_from_slice(b"\0\0\0\0\0\0");
        d.extend_from_slice(&[0, 0, 0, 0, 0, 3, 0xe8]);
        d.extend_from_slice(&[0xff; 14]);
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(b"\x0cCSTmplSampleBL\0");
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&[0xff; 16]);
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(b"\x06CSWellA12\0\0\x01\0\x0c\0\0\x02\xbc\x03\xe8Plate#1\0\0\0\0\0");
        d.extend_from_slice(&[0, 0, 0, 2]);
        d.extend_from_slice(b"\x11CSAnalysisSectionNotes#1\0");
        let (groups, wells) = template(&d).unwrap();
        assert_eq!(groups[0]["group"], "Blank");
        assert_eq!(wells.len(), 1);
        assert_eq!(
            (wells[0].row, wells[0].col, wells[0].section_id),
            (1, 12, 700)
        );
        assert_eq!(wells[0].sample, "BL");
    }

    #[test]
    fn wrong_well_count_is_refused_and_cuts_never_panic() {
        let ex = parse(&document(95)).unwrap();
        assert!(ex.blocks.is_empty());
        assert_eq!(ex.findings[0].code, "plate_not_decoded");
        let d = document(96);
        for cut in (0..d.len()).step_by(7) {
            let _ = parse(&d[..cut]);
        }
    }
}
