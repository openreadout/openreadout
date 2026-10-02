//! The `.meta` text file and the channel/scaling rules of the SpikeGLX documentation.

use std::collections::BTreeMap;

/// Parsed `key=value` lines (a leading `~` is kept in the key).
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub entries: BTreeMap<String, String>,
}

/// Parse `.meta` text: one `key=value` per line, split at the first `=`.
pub fn parse_meta(text: &str) -> Meta {
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                entries.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    Meta { entries }
}

/// Split a `~` list value `(hdr)(e1)(e2)` into its parenthesized elements.
pub fn list_elements(v: &str) -> Vec<String> {
    let t = v.trim();
    let t = t.strip_prefix('(').unwrap_or(t);
    let t = t.strip_suffix(')').unwrap_or(t);
    if t.is_empty() {
        return Vec::new();
    }
    t.split(")(").map(str::to_string).collect()
}

/// Most channels one stream may acquire or save (SpikeGLX streams have at most a few
/// thousand; the bound keeps a damaged `.meta` from sizing gigabyte tables).
const MAX_CHANNELS: usize = 65_536;

/// `a:b,c,d:e` (inclusive ranges) → indices; `all` → `0..n`. `None` for anything that is not
/// such a list or lists more than 65,536 channels.
pub fn parse_subset(v: &str, n_acquired: usize) -> Option<Vec<usize>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("all") || v.is_empty() {
        return Some((0..n_acquired).collect());
    }
    let mut out = Vec::new();
    for part in v.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once(':') {
            let a: usize = a.trim().parse().ok()?;
            let b: usize = b.trim().parse().ok()?;
            if b < a || b - a >= MAX_CHANNELS {
                return None;
            }
            out.extend(a..=b);
        } else {
            out.push(part.parse().ok()?);
        }
        // Many ranges could otherwise list billions of channels.
        if out.len() > MAX_CHANNELS {
            return None;
        }
    }
    Some(out)
}

impl Meta {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.entries.get(k).map(String::as_str)
    }
    pub fn number(&self, k: &str) -> Option<f64> {
        self.get(k)?.trim().parse().ok()
    }
    /// Comma-separated integers (`snsApLfSy=384,0,1`).
    pub fn counts(&self, k: &str) -> Option<Vec<usize>> {
        self.get(k)?
            .split(',')
            .map(|x| x.trim().parse().ok())
            .collect()
    }
}

/// Which stream a file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Neuropixels probe (`typeThis=imec`): AP or LF band plus the SY status word.
    Imec,
    /// NI-DAQ (`typeThis=nidq`): MN, MA, XA analog and XD digital words.
    Nidq,
    /// OneBox (`typeThis=obx`): XA analog, XD digital, SY status.
    Obx,
}

impl StreamKind {
    pub fn name(self) -> &'static str {
        match self {
            StreamKind::Imec => "imec",
            StreamKind::Nidq => "nidq",
            StreamKind::Obx => "obx",
        }
    }
}

/// What one saved channel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    /// Action-potential band (imec).
    Ap,
    /// Local-field-potential band (imec).
    Lf,
    /// Status word (imec, obx): raw bits.
    Sync,
    /// Multiplexed neural (NI).
    Mn,
    /// Multiplexed auxiliary (NI).
    Ma,
    /// Non-multiplexed analog (NI, OneBox).
    Xa,
    /// Digital word (NI, OneBox): raw bits.
    Xd,
}

impl ChannelKind {
    pub fn prefix(self) -> &'static str {
        match self {
            ChannelKind::Ap => "AP",
            ChannelKind::Lf => "LF",
            ChannelKind::Sync => "SY",
            ChannelKind::Mn => "MN",
            ChannelKind::Ma => "MA",
            ChannelKind::Xa => "XA",
            ChannelKind::Xd => "XD",
        }
    }
    /// True for status/digital words (no voltage scaling).
    pub fn is_bits(self) -> bool {
        matches!(self, ChannelKind::Sync | ChannelKind::Xd)
    }
}

/// One saved channel and its scaling.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedChannel {
    pub index: u32,
    /// Index among all acquired channels.
    pub acquired: usize,
    pub kind: ChannelKind,
    pub name: String,
    /// Front-end gain applied (imec AP/LF, NI MN/MA).
    pub gain: Option<f64>,
    /// `value = raw × scale` in `unit` (µV imec, V NI/OneBox; 1 and no unit for bit words).
    pub scale: f64,
    pub unit: Option<&'static str>,
}

/// Probe types whose imro table carries per-channel AP and LF gains (entry fields 3 and 4).
pub fn has_selectable_gain(probe_type: i64) -> bool {
    matches!(
        probe_type,
        0 | 1020 | 1030 | 1100 | 1120..=1123 | 1200 | 1300
    )
}

/// Fixed AP gain of probes without a per-channel gain (documented: 80 for types 21/24, 100 for
/// 2003/2013; 100 for other NP 2.0/3.0 types from the ProbeTable).
pub fn fixed_gain(probe_type: i64) -> f64 {
    match probe_type {
        21 | 24 => 80.0,
        _ => 100.0,
    }
}

/// `imMaxInt` when the metadata omits it: 512 for 10-bit NP 1.0 probes, 8192 for 14-bit NP 2.0
/// (types 21, 24), 2048 for the 12-bit NP 2.0/3.0 generation.
pub fn default_max_int(probe_type: i64) -> f64 {
    if has_selectable_gain(probe_type) || probe_type == 1110 {
        512.0
    } else if matches!(probe_type, 21 | 24) {
        8192.0
    } else {
        2048.0
    }
}

/// µV per count for an imec channel: `(Vmax / Imax) × (1 / gain) × 10⁶`.
pub fn imec_scale(range_max: f64, max_int: f64, gain: f64) -> f64 {
    (range_max / max_int) * (1.0 / gain) * 1e6
}

/// Volts per count for an NI or OneBox channel: `(1 / gain) × (Vmax / Imax)`.
pub fn ni_scale(range_max: f64, max_int: f64, gain: f64) -> f64 {
    (1.0 / gain) * (range_max / max_int)
}

/// Resolve the saved channels of a stream: kind, name (from `~snsChanMap`), gain and scale.
pub fn saved_channels(meta: &Meta, kind: StreamKind) -> Result<Vec<SavedChannel>, String> {
    let n_saved = meta
        .number("nSavedChans")
        .ok_or("no nSavedChans in the .meta")? as usize;
    if n_saved == 0 || n_saved > MAX_CHANNELS {
        return Err(format!("nSavedChans={n_saved}"));
    }
    let names: Vec<String> = meta
        .get("~snsChanMap")
        .map(list_elements)
        .unwrap_or_default()
        .into_iter()
        .skip(1)
        .map(|e| e.split(';').next().unwrap_or("").to_string())
        .collect();
    // acquired layout: kinds by acquired index
    let (acq_key, kinds): (&str, Vec<ChannelKind>) = match kind {
        StreamKind::Imec => (
            "acqApLfSy",
            vec![ChannelKind::Ap, ChannelKind::Lf, ChannelKind::Sync],
        ),
        StreamKind::Nidq => (
            "acqMnMaXaDw",
            vec![
                ChannelKind::Mn,
                ChannelKind::Ma,
                ChannelKind::Xa,
                ChannelKind::Xd,
            ],
        ),
        StreamKind::Obx => (
            "acqXaDwSy",
            vec![ChannelKind::Xa, ChannelKind::Xd, ChannelKind::Sync],
        ),
    };
    let acq = meta
        .counts(acq_key)
        .ok_or_else(|| format!("no {acq_key} in the .meta"))?;
    let total = acq
        .iter()
        .take(kinds.len())
        .fold(0usize, |a, &n| a.saturating_add(n));
    if total > MAX_CHANNELS {
        return Err(format!("{acq_key} adds up to {total} acquired channels"));
    }
    let mut by_acq = Vec::new();
    for (k, &n) in kinds.iter().zip(acq.iter()) {
        by_acq.extend(std::iter::repeat_n(*k, n));
    }
    let subset = parse_subset(meta.get("snsSaveChanSubset").unwrap_or("all"), by_acq.len())
        .ok_or("snsSaveChanSubset is not a channel list")?;
    if subset.len() != n_saved {
        return Err(format!(
            "snsSaveChanSubset lists {} channels but nSavedChans={n_saved}",
            subset.len()
        ));
    }
    let probe_type = meta.number("imDatPrb_type").map_or(0, |v| v as i64);
    let imro: Vec<Vec<f64>> = meta
        .get("~imroTbl")
        .map(list_elements)
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            e.split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect()
        })
        .collect();
    let range_max = meta.number(match kind {
        StreamKind::Imec => "imAiRangeMax",
        StreamKind::Nidq => "niAiRangeMax",
        StreamKind::Obx => "obAiRangeMax",
    });
    let max_int = match kind {
        StreamKind::Imec => meta
            .number("imMaxInt")
            .unwrap_or_else(|| default_max_int(probe_type)),
        StreamKind::Nidq => meta.number("niMaxInt").unwrap_or(32_768.0),
        StreamKind::Obx => meta.number("obMaxInt").unwrap_or(32_768.0),
    };
    let n_ap = acq.first().copied().unwrap_or(0);
    let mut out = Vec::with_capacity(n_saved);
    let mut counters: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (i, &a) in subset.iter().enumerate() {
        let ck = *by_acq.get(a).ok_or_else(|| {
            format!("saved channel {i} is acquired channel {a}, beyond the acquired layout")
        })?;
        let within = match ck {
            ChannelKind::Lf => a - n_ap,
            _ => a,
        };
        let gain = match (kind, ck) {
            (StreamKind::Imec, ChannelKind::Ap | ChannelKind::Lf) => {
                let field = if ck == ChannelKind::Ap { 3 } else { 4 };
                let from_imro = if has_selectable_gain(probe_type) {
                    imro.get(1 + within).and_then(|e| e.get(field)).copied()
                } else {
                    None
                };
                let from_key = meta.number(if ck == ChannelKind::Ap {
                    "imChan0apGain"
                } else {
                    "imChan0lfGain"
                });
                let from_1110 = if probe_type == 1110 {
                    imro.first().and_then(|h| h.get(field)).copied()
                } else {
                    None
                };
                Some(
                    from_imro
                        .or(from_key)
                        .or(from_1110)
                        .unwrap_or_else(|| fixed_gain(probe_type)),
                )
            }
            (StreamKind::Nidq, ChannelKind::Mn) => Some(meta.number("niMNGain").unwrap_or(1.0)),
            (StreamKind::Nidq, ChannelKind::Ma) => Some(meta.number("niMAGain").unwrap_or(1.0)),
            _ => None,
        };
        let (scale, unit) = if ck.is_bits() {
            (1.0, None)
        } else {
            match (kind, range_max) {
                (StreamKind::Imec, Some(r)) => {
                    (imec_scale(r, max_int, gain.unwrap_or(1.0)), Some("µV"))
                }
                (_, Some(r)) => (ni_scale(r, max_int, gain.unwrap_or(1.0)), Some("V")),
                (_, None) => (1.0, None),
            }
        };
        let seq = counters.entry(ck.prefix()).or_insert(0);
        let fallback = format!("{}{}", ck.prefix(), *seq);
        *seq += 1;
        let name = names
            .get(i)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or(fallback);
        out.push(SavedChannel {
            index: i as u32,
            acquired: a,
            kind: ck,
            name,
            gain,
            scale,
            unit,
        });
    }
    Ok(out)
}

/// The saved neural channels' sites: `~snsGeomMap` (µm, SpikeGLX 20230202 and later) or the
/// older `~snsShankMap` (grid indices).
#[derive(Debug, Clone, PartialEq)]
pub struct SiteMap {
    /// `~snsGeomMap` or `~snsShankMap`.
    pub source: &'static str,
    /// Geometry map: the probe part number.
    pub part_number: Option<String>,
    pub shanks: u32,
    /// Geometry map: spacing between shanks and the width of one shank, µm.
    pub shank_pitch_um: Option<f64>,
    pub shank_width_um: Option<f64>,
    /// Shank map: the grid's columns and rows (maxima; not every cell holds an electrode).
    pub columns: Option<u32>,
    pub rows: Option<u32>,
    /// One per saved neural channel, in saved order.
    pub sites: Vec<ChannelSite>,
}

/// One saved neural channel's electrode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelSite {
    /// Zero-based shank, shank 0 left-most with the tips pointing down.
    pub shank: u32,
    /// Geometry map: the electrode centre from the shank's left edge (x) and from the centre of
    /// the bottom-most electrode row (z), µm, in the shank's own frame.
    pub x_um: Option<f64>,
    pub z_um: Option<f64>,
    /// Shank map: the electrode's column and row.
    pub col: Option<u32>,
    pub row: Option<u32>,
    /// The map's "used" flag (drawn in the viewers and included in spatial averages).
    pub used: bool,
}

/// Parse the stream's site map for `n_neural` saved neural channels (imec AP and LF, NI MN).
/// `None` without a map or when the map is not one well-formed entry per saved neural channel
/// (no geometry is then reported, rather than a misaligned one).
pub fn site_map(meta: &Meta, n_neural: usize) -> Option<SiteMap> {
    if n_neural == 0 {
        return None;
    }
    let (source, value) = if let Some(value) = meta.get("~snsGeomMap") {
        ("~snsGeomMap", value)
    } else {
        ("~snsShankMap", meta.get("~snsShankMap")?)
    };
    let elements = list_elements(value);
    let (header, entries) = elements.split_first()?;
    if entries.len() != n_neural {
        return None;
    }
    let head: Vec<&str> = header.split(',').map(str::trim).collect();
    let geom = source == "~snsGeomMap";
    let mut map = SiteMap {
        source,
        part_number: None,
        shanks: 0,
        shank_pitch_um: None,
        shank_width_um: None,
        columns: None,
        rows: None,
        sites: Vec::with_capacity(entries.len()),
    };
    if geom {
        let [pn, shank_count, pitch, width] = head.as_slice() else {
            return None;
        };
        map.part_number = Some((*pn).to_string());
        map.shanks = shank_count.parse().ok()?;
        map.shank_pitch_um = Some(pitch.parse::<f64>().ok().filter(|x| x.is_finite())?);
        map.shank_width_um = Some(width.parse::<f64>().ok().filter(|x| x.is_finite())?);
    } else {
        let [shank_count, cols, rows] = head.as_slice() else {
            return None;
        };
        map.shanks = shank_count.parse().ok()?;
        map.columns = Some(cols.parse().ok()?);
        map.rows = Some(rows.parse().ok()?);
    }
    for entry in entries {
        let fields: Vec<&str> = entry.split(':').map(str::trim).collect();
        let [sh, a, b, in_use] = fields.as_slice() else {
            return None;
        };
        let shank: u32 = sh.parse().ok()?;
        if shank >= map.shanks.max(1) {
            return None;
        }
        let used = match *in_use {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        map.sites.push(if geom {
            ChannelSite {
                shank,
                x_um: Some(a.parse::<f64>().ok().filter(|x| x.is_finite())?),
                z_um: Some(b.parse::<f64>().ok().filter(|x| x.is_finite())?),
                col: None,
                row: None,
                used,
            }
        } else {
            ChannelSite {
                shank,
                x_um: None,
                z_um: None,
                col: Some(a.parse().ok()?),
                row: Some(b.parse().ok()?),
                used,
            }
        });
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_maps() {
        let m = parse_meta("~snsGeomMap=(NP2013,4,250,70)(2:27:720:1)(3:59:735:0)\n");
        let g = site_map(&m, 2).unwrap();
        assert_eq!(
            (g.shanks, g.shank_pitch_um, g.part_number.as_deref()),
            (4, Some(250.0), Some("NP2013"))
        );
        assert_eq!(
            g.sites[1],
            ChannelSite {
                shank: 3,
                x_um: Some(59.0),
                z_um: Some(735.0),
                col: None,
                row: None,
                used: false
            }
        );
        // one entry per saved neural channel, or nothing
        assert_eq!(site_map(&m, 3), None);
        let m = parse_meta("~snsShankMap=(1,2,480)(0:0:0:1)(0:1:0:1)(0:0:1:1)\n");
        let s = site_map(&m, 3).unwrap();
        assert_eq!(
            (s.columns, s.rows, s.sites[2].col, s.sites[2].row),
            (Some(2), Some(480), Some(0), Some(1))
        );
        // a shank beyond the header's count, a bad flag
        assert_eq!(
            site_map(&parse_meta("~snsShankMap=(1,2,4)(1:0:0:1)\n"), 1),
            None
        );
        assert_eq!(
            site_map(&parse_meta("~snsShankMap=(1,2,4)(0:0:0:7)\n"), 1),
            None
        );
    }

    #[test]
    fn subsets_and_lists() {
        assert_eq!(parse_subset("0:2,5", 9), Some(vec![0, 1, 2, 5]));
        assert_eq!(parse_subset("all", 3), Some(vec![0, 1, 2]));
        assert_eq!(parse_subset("3:1", 9), None);
        assert_eq!(
            list_elements("(384,384,1)(AP0;0:0)(AP1;1:1)"),
            vec!["384,384,1", "AP0;0:0", "AP1;1:1"]
        );
    }

    #[test]
    fn channel_counts_are_bounded() {
        assert_eq!(parse_subset("0:65535,0:65535", 9), None);
        assert_eq!(parse_subset("0:4000000000", 9), None);
        let m = parse_meta(
            "typeThis=imec\nnSavedChans=1\nacqApLfSy=4000000000,0,1\nsnsSaveChanSubset=0\n",
        );
        assert!(saved_channels(&m, StreamKind::Imec).is_err());
    }

    #[test]
    fn np1_imro_gains() {
        let m = parse_meta(
            "typeThis=imec\nnSavedChans=3\nacqApLfSy=2,2,1\nsnsSaveChanSubset=0,3,4\nimDatPrb_type=0\nimAiRangeMax=0.6\n~imroTbl=(0,2)(0 0 0 500 250 1)(1 0 0 1000 125 1)\n~snsChanMap=(2,2,1)(AP0;0:0)(LF1;3:3)(SY0;4:4)\n",
        );
        let c = saved_channels(&m, StreamKind::Imec).unwrap();
        assert_eq!(c[0].kind, ChannelKind::Ap);
        assert_eq!(c[1].kind, ChannelKind::Lf);
        assert_eq!(c[1].gain, Some(125.0));
        assert!((c[0].scale - 0.6 / 512.0 / 500.0 * 1e6).abs() < 1e-12);
        assert_eq!(c[2].kind, ChannelKind::Sync);
        assert_eq!(c[2].unit, None);
        assert_eq!(c[1].name, "LF1");
    }

    #[test]
    fn nidq_gains() {
        let m = parse_meta(
            "typeThis=nidq\nnSavedChans=4\nacqMnMaXaDw=1,1,1,1\nsnsSaveChanSubset=all\nniAiRangeMax=5\nniMNGain=200\nniMAGain=2\n",
        );
        let c = saved_channels(&m, StreamKind::Nidq).unwrap();
        assert!((c[0].scale - 5.0 / 32768.0 / 200.0).abs() < 1e-15);
        assert!((c[1].scale - 5.0 / 32768.0 / 2.0).abs() < 1e-15);
        assert_eq!(c[3].kind, ChannelKind::Xd);
        assert_eq!(c[3].name, "XD0");
    }
}
