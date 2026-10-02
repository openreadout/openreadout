//! What a Shimadzu `Chromatogram ChN` stream is: the detector it comes from, the name the
//! vendor's ASCII export gives it, and the factor from stored integers to the export's units
//! (`docs/formats/shimadzu.md` § Channels; provenance 2026-09-24).
//!
//! - Older layout (`LC Raw Data`): `LC Raw Data/Chromatogram Status`, one 64-byte record per
//!   channel (module code, factor, divisor, wavelength), and `LSS Configuration/LC Configuration`,
//!   128-byte module records (code, serial, firmware, model, user name).
//! - Newer layout (`LSS Raw Data`): `LSS Raw Data/2D Data Item`, XML with one `DII` per channel
//!   (`DT`, `DK` 0 chromatogram / 2 status log, `CN`, `DN`, `DETN`, `DSN`, `CF`, `ATN` = the
//!   export's section title). `DT` is 52 for LabSolutions 5.1 chromatograms but 48 for
//!   chromatograms and status logs alike in a later writer, so the title decides.

use openreadout_core::bytes::{latin1_field, le_f32, le_f64, le_u16};

/// Bytes of one `Chromatogram Status` record.
pub const STATUS_RECORD: usize = 64;
/// Bytes of one `LC Configuration` module record.
pub const MODULE_RECORD: usize = 128;
/// `DII/DT` of an LC chromatogram channel in LabSolutions 5.1's `2D Data Item` (a GC
/// chromatogram has 18; a later LC writer has 48, the status logs' value).
pub const DATA_ITEM_CHROMATOGRAM: u32 = 52;
/// `DII/DT` of a status log in `2D Data Item`.
pub const DATA_ITEM_STATUS: u32 = 48;

/// One channel record of `LC Raw Data/Chromatogram Status`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelStatus {
    /// 1-based channel number (record index + 1).
    pub channel: u32,
    /// Code of the module (detector) the channel belongs to (`LC Configuration`).
    pub module_code: u16,
    /// Factor from stored integers to the export's `Intensity` column.
    pub factor: f64,
    /// A second factor (1.0 in the corpus), applied with `factor`.
    pub factor2: f64,
    /// Divisor from `Intensity` to the export's units (1000: `Intensity Multiplier` 0.001).
    pub divisor: f64,
    /// Detection wavelength, nm (UV detectors).
    pub wavelength_nm: Option<f64>,
    /// Unit of the scaled values, as the record writes it at +40 (`mV`, `uV`, `bar`, `C`).
    pub unit: Option<String>,
}

impl ChannelStatus {
    /// Stored integer → export units (mV), when the record is plausible.
    pub fn scale(&self) -> Option<f64> {
        let s = self.factor * self.factor2 / self.divisor;
        (s.is_finite() && s != 0.0 && self.divisor > 0.0).then_some(s)
    }
}

/// The records of `LC Raw Data/Chromatogram Status` that hold a factor.
pub fn channel_statuses(b: &[u8]) -> Vec<ChannelStatus> {
    let mut out = Vec::new();
    for (i, r) in b.as_chunks::<STATUS_RECORD>().0.iter().enumerate() {
        let (Some(code), Some(factor), Some(factor2), Some(divisor)) =
            (le_u16(r, 4), le_f64(r, 16), le_f64(r, 24), le_f64(r, 32))
        else {
            continue;
        };
        if !factor.is_finite() || factor == 0.0 || !divisor.is_finite() || divisor <= 0.0 {
            continue;
        }
        let wl = le_f32(r, 58)
            .map(f64::from)
            .filter(|w| w.is_finite() && (100.0..=1200.0).contains(w));
        let unit = Some(cstr(r, 40, 18))
            .filter(|u| !u.is_empty() && u.chars().all(|c| c.is_ascii_graphic()))
            .map(|u| display_unit(&u));
        out.push(ChannelStatus {
            channel: u32::try_from(i + 1).unwrap_or(u32::MAX),
            module_code: code,
            factor,
            factor2: if factor2.is_finite() && factor2 != 0.0 {
                factor2
            } else {
                1.0
            },
            divisor,
            wavelength_nm: wl,
            unit,
        });
    }
    out
}

/// A record's unit as we write it (`uV` → `µV`, `C` → `°C`).
pub fn display_unit(u: &str) -> String {
    match u {
        "uV" => "µV".into(),
        "uAU" => "µAU".into(),
        "C" => "°C".into(),
        _ => u.to_string(),
    }
}

/// What a status log measures, from its unit.
pub fn quantity(unit: Option<&str>) -> &'static str {
    match unit.map(str::to_ascii_lowercase).as_deref() {
        Some("kgf/cm2" | "bar" | "mpa" | "psi" | "kpa") => "pressure",
        Some("°c" | "c") => "temperature",
        _ => "status",
    }
}

/// One module record of `LSS Configuration/LC Configuration`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LcModule {
    pub code: u16,
    pub serial: String,
    pub firmware: String,
    pub model: String,
    /// The user's name for the module (`Detector A`, `AD1`); empty for pumps and ovens.
    pub name: String,
}

fn cstr(b: &[u8], at: usize, len: usize) -> String {
    latin1_field(b.get(at..(at + len).min(b.len())).unwrap_or_default())
}

/// The module records of `LSS Configuration/LC Configuration` (records with a model or name).
pub fn lc_modules(b: &[u8]) -> Vec<LcModule> {
    b.as_chunks::<MODULE_RECORD>()
        .0
        .iter()
        .filter_map(|r| {
            let m = LcModule {
                code: le_u16(r, 0)?,
                serial: cstr(r, 8, 16),
                firmware: cstr(r, 24, 16),
                model: cstr(r, 56, 16),
                name: cstr(r, 72, 32),
            };
            (!m.model.is_empty() || !m.name.is_empty()).then_some(m)
        })
        .collect()
}

/// One `DII` of `LSS Raw Data/2D Data Item`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataItem {
    /// `DT`: 52 chromatogram, 48 status log.
    pub data_type: u32,
    /// `CN`: channel number (`Chromatogram Ch<CN>`, `StatusLog Ch<CN>`).
    pub channel: u32,
    /// `DN`: display name.
    pub name: String,
    /// `DETN`: detector name.
    pub detector: String,
    /// `DSN`: device (module model).
    pub device: String,
    /// `ATN`: the vendor's ASCII-export section title, without brackets.
    pub export_section: String,
    /// `CF`: conversion factor (little-endian f64 written in hex).
    pub factor: Option<f64>,
    /// `DSID`: data set id, which names the item's peak table (`PT-<DSID>`).
    pub data_set: String,
}

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(&s[a..b])
}

/// A little-endian f64 written as 16 hex digits (`9A9999999999B93F` = 0.1).
pub fn hex_f64(h: &str) -> Option<f64> {
    let h = h.trim();
    if h.len() != 16 {
        return None;
    }
    let mut b = [0u8; 8];
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = u8::from_str_radix(h.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(f64::from_le_bytes(b)).filter(|v| v.is_finite())
}

/// The `DII` entries of a `2D Data Item` text.
pub fn data_items(xml: &str) -> Vec<DataItem> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(a) = rest.find("<DII") {
        let Some(b) = rest[a..].find("</DII>") else {
            break;
        };
        let item = &rest[a..a + b];
        let num = |t: &str| tag(item, t).and_then(|v| v.trim().parse::<u32>().ok());
        let txt = |t: &str| {
            tag(item, t)
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        };
        if let (Some(dt), Some(cn)) = (num("DT"), num("CN")) {
            out.push(DataItem {
                data_type: dt,
                channel: cn,
                name: txt("DN"),
                detector: txt("DETN"),
                device: txt("DSN"),
                export_section: txt("ATN")
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string(),
                factor: tag(item, "CF").and_then(hex_f64),
                data_set: txt("DSID"),
            });
        }
        rest = &rest[a + b + 6..];
    }
    out
}

/// Who a chromatogram channel is, as far as the file says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelLabel {
    /// Display name, as the vendor's export names the channel (`Detector A-Ch1`, `AD1`).
    pub name: String,
    /// The export's section title (`LC Chromatogram(Detector A-Ch1)`).
    pub export_section: String,
    pub detector_name: Option<String>,
    pub detector_model: Option<String>,
    pub wavelength_nm: Option<f64>,
    /// Stored integer → reported value (older layout: to mV).
    pub scale: Option<f64>,
    pub unit: Option<String>,
}

/// Labels of the older layout's channels `channels` (1-based, in stream order).
pub fn label_lc_channels(
    channels: &[u32],
    statuses: &[ChannelStatus],
    modules: &[LcModule],
) -> Vec<Option<ChannelLabel>> {
    let mut per_module: std::collections::BTreeMap<u16, u32> = std::collections::BTreeMap::new();
    channels
        .iter()
        .map(|ch| {
            let st = statuses.iter().find(|s| s.channel == *ch)?;
            let same: Vec<&LcModule> = modules
                .iter()
                .filter(|m| m.code == st.module_code)
                .collect();
            let module = match same.as_slice() {
                [m] => Some(*m),
                _ => None,
            };
            let base = module.map(|m| m.name.clone()).filter(|n| !n.is_empty());
            let name = base.as_ref().map(|b| {
                if b.starts_with("Detector") {
                    let k = per_module.entry(st.module_code).or_insert(0);
                    *k += 1;
                    format!("{b}-Ch{k}")
                } else {
                    b.clone()
                }
            });
            Some(ChannelLabel {
                export_section: name
                    .as_ref()
                    .map(|n| format!("LC Chromatogram({n})"))
                    .unwrap_or_default(),
                name: name.unwrap_or_default(),
                detector_name: base,
                detector_model: module.map(|m| m.model.clone()).filter(|m| !m.is_empty()),
                wavelength_nm: st.wavelength_nm,
                scale: st.scale(),
                unit: st
                    .scale()
                    .map(|_| st.unit.clone().unwrap_or_else(|| "mV".to_string())),
            })
        })
        .collect()
}

/// Labels of the older layout's status logs: the file stores no names, so a log is named by
/// its module's model, what it measures (from the unit) and, when the module has several of
/// the kind, a count (`LC-20AD pressure 1`).
pub fn label_lc_status(
    channels: &[u32],
    statuses: &[ChannelStatus],
    modules: &[LcModule],
) -> Vec<Option<ChannelLabel>> {
    let named: Vec<Option<(String, &ChannelStatus)>> = channels
        .iter()
        .map(|ch| {
            let st = statuses.iter().find(|s| s.channel == *ch)?;
            let model = modules
                .iter()
                .find(|m| m.code == st.module_code && !m.model.is_empty())
                .map_or_else(
                    || format!("module 0x{:02X}", st.module_code),
                    |m| m.model.clone(),
                );
            Some((format!("{model} {}", quantity(st.unit.as_deref())), st))
        })
        .collect();
    let mut seen: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    named
        .iter()
        .map(|n| {
            let (base, st) = n.as_ref()?;
            let total = named.iter().flatten().filter(|(b, _)| b == base).count();
            let k = seen.entry(base.clone()).or_insert(0);
            *k += 1;
            let name = if total > 1 {
                format!("{base} {k}")
            } else {
                base.clone()
            };
            Some(ChannelLabel {
                name,
                export_section: String::new(),
                detector_name: None,
                detector_model: base.rsplit_once(' ').map(|(m, _)| m.to_string()),
                wavelength_nm: None,
                scale: st.scale(),
                unit: st.unit.clone(),
            })
        })
        .collect()
}

/// Labels of the newer layout's status logs: `2D Data Item` entries with `DT` 48 name them
/// (the export's `LC Status Trace(...)` title), `StatusLog Status` records scale them.
pub fn label_lss_status(
    channels: &[u32],
    items: &[DataItem],
    statuses: &[ChannelStatus],
) -> Vec<Option<ChannelLabel>> {
    channels
        .iter()
        .map(|ch| {
            let st = statuses.iter().find(|s| s.channel == *ch)?;
            let it = items.iter().find(|i| is_status_item(i) && i.channel == *ch);
            let section = it.map(|i| i.export_section.clone()).unwrap_or_default();
            let name = section
                .strip_prefix("LC Status Trace(")
                .and_then(|s| s.strip_suffix(')'))
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .or_else(|| it.map(|i| i.name.clone()).filter(|n| !n.is_empty()))
                .unwrap_or_else(|| format!("status Ch{ch} {}", quantity(st.unit.as_deref())));
            Some(ChannelLabel {
                name,
                export_section: section,
                detector_name: None,
                detector_model: it.map(|i| i.device.clone()).filter(|d| !d.is_empty()),
                wavelength_nm: None,
                scale: st.scale(),
                unit: st.unit.clone(),
            })
        })
        .collect()
}

/// Whether a `2D Data Item` entry is a chromatogram: its export title names a chromatogram
/// (`[LC Chromatogram(Detector A-Ch1)]`, a GC `.gcd`'s `[Chromatogram (Ch1)]`) and not a status
/// trace. The `DT` does not decide it: LabSolutions 5.1 writes 52 for chromatograms, a later
/// writer (file version 5.01, LCMS-8060NX) 48 for chromatograms and status logs alike, with
/// `DK` 0 and 2.
pub fn is_chromatogram_item(i: &DataItem) -> bool {
    i.export_section.contains("Chromatogram") && !i.export_section.contains("Status Trace(")
}

/// Whether a `2D Data Item` entry is a status log: `DT` 48 and a `Status Trace` title (every
/// status entry of the corpus has both).
pub fn is_status_item(i: &DataItem) -> bool {
    i.data_type == DATA_ITEM_STATUS && i.export_section.contains("Status Trace(")
}

/// Labels of the newer layout's channels from `2D Data Item` (chromatogram items with the
/// channel's number) and the scale and unit of `LSS Raw Data/Chromatogram Status`.
pub fn label_lss_channels(
    channels: &[u32],
    items: &[DataItem],
    statuses: &[ChannelStatus],
) -> Vec<Option<ChannelLabel>> {
    channels
        .iter()
        .map(|ch| {
            let it = items
                .iter()
                .find(|i| is_chromatogram_item(i) && i.channel == *ch)?;
            let section = it.export_section.clone();
            let name = section
                .strip_prefix("LC Chromatogram(")
                .and_then(|s| s.strip_suffix(')'))
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .or_else(|| (!it.name.is_empty()).then(|| it.name.clone()))?;
            let st = statuses.iter().find(|s| s.channel == *ch);
            Some(ChannelLabel {
                name,
                export_section: section,
                detector_name: (!it.detector.is_empty()).then(|| it.detector.clone()),
                detector_model: (!it.device.is_empty()).then(|| it.device.clone()),
                wavelength_nm: st.and_then(|s| s.wavelength_nm),
                scale: st.and_then(ChannelStatus::scale),
                unit: st.and_then(|s| s.scale().and(s.unit.clone())),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16, factor: f64, wl: f32) -> Vec<u8> {
        let mut r = vec![0u8; STATUS_RECORD];
        r[0] = 1;
        r[4..6].copy_from_slice(&code.to_le_bytes());
        r[16..24].copy_from_slice(&factor.to_le_bytes());
        r[24..32].copy_from_slice(&1.0f64.to_le_bytes());
        r[32..40].copy_from_slice(&1000.0f64.to_le_bytes());
        r[58..62].copy_from_slice(&wl.to_le_bytes());
        r
    }

    fn module(code: u16, model: &str, name: &str) -> Vec<u8> {
        let mut r = vec![0u8; MODULE_RECORD];
        r[..2].copy_from_slice(&code.to_le_bytes());
        r[56..56 + model.len()].copy_from_slice(model.as_bytes());
        r[72..72 + name.len()].copy_from_slice(name.as_bytes());
        r
    }

    #[test]
    fn older_layout_labels() {
        let mut st = status(0x34, 10000.0 / 2_097_152.0, 280.0);
        st.extend(vec![0u8; STATUS_RECORD * 3]);
        st.extend(status(0x01, 0.2, 0.0));
        let statuses = channel_statuses(&st);
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[1].channel, 5);
        let mut m = module(0x1c, "LC-20AD", "");
        m.extend(module(0x34, "SPD-20AV", "Detector A"));
        m.extend(module(0x01, "", "AD1"));
        let modules = lc_modules(&m);
        let l = label_lc_channels(&[1, 5, 3], &statuses, &modules);
        let a = l[0].as_ref().unwrap();
        assert_eq!(a.name, "Detector A-Ch1");
        assert_eq!(a.export_section, "LC Chromatogram(Detector A-Ch1)");
        assert_eq!(a.detector_model.as_deref(), Some("SPD-20AV"));
        assert_eq!(a.wavelength_nm, Some(280.0));
        assert!((a.scale.unwrap() - 0.004_768_371_582_031_25e-3).abs() < 1e-18);
        assert_eq!(l[1].as_ref().unwrap().name, "AD1");
        assert!(l[2].is_none());
    }

    #[test]
    fn chromatogram_items_with_dt_48() {
        let xml = "<GUD><DII><DT>48</DT><DK>2</DK><CN>1</CN><DN>A</DN><ATN>[LC Status Trace(Pump A Pressure)]</ATN></DII><DII><DT>48</DT><DK>0</DK><CN>1</CN><DN>A Ch1</DN><ATN>[LC Chromatogram(Detector A-Ch1)]</ATN></DII><DII><DT>48</DT><DK>2</DK><CN>8</CN><DN>B</DN><ATN>[LC Status Trace(UV Cell Temp.)]</ATN></DII></GUD>";
        let items = data_items(xml);
        assert!(!is_chromatogram_item(&items[0]));
        assert!(is_chromatogram_item(&items[1]));
        assert!(!is_chromatogram_item(&items[2]));
        let mut st = vec![0u8; STATUS_RECORD];
        st[0] = 1;
        st[16..24].copy_from_slice(&(20_000.0f64 / 4_194_304.0).to_le_bytes());
        st[24..32].copy_from_slice(&1.0f64.to_le_bytes());
        st[32..40].copy_from_slice(&1000.0f64.to_le_bytes());
        st[40..42].copy_from_slice(b"mV");
        let l = label_lss_channels(&[1], &items, &channel_statuses(&st));
        let a = l[0].as_ref().unwrap();
        assert_eq!(a.name, "Detector A-Ch1");
        assert_eq!(a.unit.as_deref(), Some("mV"));
        assert!((a.scale.unwrap() - 20.0 / 4_194_304.0).abs() < 1e-15);
    }

    #[test]
    fn newer_layout_labels() {
        let xml = "<GUD><DII><DT>48</DT><CN>1</CN><CF>9A9999999999B93F</CF><DN>Pump A Pressure</DN><ATN>[LC Status Trace(Pump A Pressure)]</ATN></DII><DII><DT>52</DT><DK>0</DK><CN>1</CN><CF>9A999999FF6968BF</CF><DN>AD1</DN><DSN>AD</DSN><DETN>AD1</DETN><ATN>[LC Chromatogram(AD1)]</ATN></DII></GUD>";
        let items = data_items(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].factor, Some(0.1));
        let l = label_lss_channels(&[1, 2], &items, &[]);
        let a = l[0].as_ref().unwrap();
        assert_eq!(a.name, "AD1");
        assert_eq!(a.export_section, "LC Chromatogram(AD1)");
        assert_eq!(a.detector_model.as_deref(), Some("AD"));
        assert!(l[1].is_none());
        assert_eq!(hex_f64("zz"), None);
        assert!(data_items("<DII><DT>x").is_empty());
    }
}
