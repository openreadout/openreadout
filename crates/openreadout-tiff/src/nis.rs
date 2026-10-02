//! Nikon NIS-Elements TIFF exports: the private double-valued tags on page 0 and the index
//! tokens at the end of the file names that tie one export's files together. Derived from
//! corpus files and the depositors' descriptions; see `docs/formats/tiff.md` § NIS-Elements
//! and `docs/provenance/tiff.md`.

use std::collections::HashMap;
use std::path::PathBuf;

use openreadout_core::Result;
use openreadout_core::bytes::utf16le;
use openreadout_core::model::{ChannelInfo, ImageInfo, InstrumentInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use serde_json::{Map, Value, json};

use crate::container::{FieldValue, Ifd};
use crate::dataset::{PlaneSrc, Series, TiffDataset, check_plane_count, dir_of, resolution_um};
use crate::decode::PageLayout;
use crate::files::{FileSetMember, file_name_of};

/// Private tag: time since the acquisition started (ms).
pub const NIS_TAG_TIME: u16 = 65325;
/// Private tag: pixel size (µm).
pub const NIS_TAG_PIXEL_SIZE: u16 = 65326;
/// Private tag: stage X, Y, Z (µm) and time (ms).
pub const NIS_TAG_STAGE: u16 = 65329;
/// Private tag: the named metadata block (`MetadataTiffV1_0`, LV-encoded; not decoded).
pub const NIS_TAG_METADATA: u16 = 65331;

/// Values of the NIS-Elements private tags of one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NisPage {
    /// Time since the acquisition started, ms.
    pub time_ms: Option<f64>,
    /// Pixel size, µm (inferred meaning).
    pub pixel_size_um: Option<f64>,
    /// Stage X, Y, Z, µm.
    pub stage_um: Option<[f64; 3]>,
    /// Tags 65327 and 65328 as found (1.0 in the corpus; meaning unknown).
    pub unknown: [Option<f64>; 2],
}

fn doubles(ifd: &Ifd, tag: u16) -> Option<&[f64]> {
    match &ifd.field(tag)?.value {
        FieldValue::Float(v) if ifd.field(tag)?.field_type == 12 => Some(v.as_slice()),
        _ => None,
    }
}

/// `None` unless page 0 carries the NIS-Elements pixel-size tag and the named metadata block.
pub fn parse_nis(ifd: &Ifd) -> Option<NisPage> {
    let meta = ifd.bytes(NIS_TAG_METADATA)?;
    // u32 count, u32 name length, then the UTF-16 name `MetadataTiffV1_0`
    if utf16le(meta.get(8..40)?) != "MetadataTiffV1_0" {
        return None;
    }
    let first = |tag| doubles(ifd, tag).and_then(|v| v.first().copied());
    let pixel = first(NIS_TAG_PIXEL_SIZE).filter(|v| v.is_finite() && *v > 0.0);
    let stage = doubles(ifd, NIS_TAG_STAGE)
        .filter(|v| v.len() >= 3 && v[..3].iter().all(|x| x.is_finite()))
        .map(|v| [v[0], v[1], v[2]]);
    Some(NisPage {
        time_ms: first(NIS_TAG_TIME).filter(|v| v.is_finite()),
        pixel_size_um: pixel,
        stage_um: stage,
        unknown: [first(65327), first(65328)],
    })
}

/// An index token of an exported file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SequenceAxis {
    /// `xy<n>`: stage position.
    Position,
    /// `t<n>`: time point.
    Time,
    /// `z<n>`: focal plane.
    Z,
    /// `c<n>`: channel.
    Channel,
}

/// A file name split into the export's prefix and its index tokens (in name order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceName {
    /// Everything before the tokens.
    pub prefix: String,
    /// The tokens: axis and number, in the order they appear.
    pub indices: Vec<(SequenceAxis, u32)>,
    /// Extension, lower case.
    pub extension: String,
}

/// Split `name` (a file name with extension) into prefix and index tokens, read from the end
/// of the stem: `single_plane_1-2000_2xy01c1.tif` → prefix `single_plane_1-2000_2`,
/// `[(Position, 1), (Channel, 1)]`. `None` without tokens or without a prefix.
pub fn parse_sequence_name(name: &str) -> Option<SequenceName> {
    let (stem, ext) = name.rsplit_once('.')?;
    let b = stem.as_bytes();
    let mut end = b.len();
    let mut tokens = Vec::new();
    loop {
        let digits_start = b[..end]
            .iter()
            .rposition(|c| !c.is_ascii_digit())
            .map_or(0, |p| p + 1);
        if digits_start == end || digits_start == 0 || end - digits_start > 9 {
            break;
        }
        let n: u32 = stem[digits_start..end].parse().ok()?;
        let (axis, len) = match b[digits_start - 1] {
            b'y' | b'Y' if digits_start >= 2 && matches!(b[digits_start - 2], b'x' | b'X') => {
                (SequenceAxis::Position, 2)
            }
            b't' | b'T' => (SequenceAxis::Time, 1),
            b'z' | b'Z' => (SequenceAxis::Z, 1),
            b'c' | b'C' => (SequenceAxis::Channel, 1),
            _ => break,
        };
        if digits_start - len == 0 {
            break; // the whole stem would be tokens: no prefix
        }
        tokens.push((axis, n));
        end = digits_start - len;
    }
    if tokens.is_empty() {
        return None;
    }
    tokens.reverse();
    let mut axes: Vec<SequenceAxis> = tokens.iter().map(|t| t.0).collect();
    axes.sort_unstable();
    axes.dedup();
    if axes.len() != tokens.len() {
        return None; // an axis twice: not an export name
    }
    Some(SequenceName {
        prefix: stem[..end].to_string(),
        indices: tokens,
        extension: ext.to_ascii_lowercase(),
    })
}

impl SequenceName {
    /// Index of `axis`, if the name has one.
    pub fn index(&self, axis: SequenceAxis) -> Option<u32> {
        self.indices.iter().find(|t| t.0 == axis).map(|t| t.1)
    }

    /// The axes in name order (files of one export share them).
    pub fn axes(&self) -> Vec<SequenceAxis> {
        self.indices.iter().map(|t| t.0).collect()
    }
}

/// Largest number of files grouped into one NIS-Elements export data set.
const MAX_NIS_FILES: usize = 100_000;

/// The distinct index numbers of each axis over an export's files (`[0]` for an axis no
/// file name has), and the member of each (position, c, z, t) by their positions in those.
struct NisGrid {
    positions: Vec<u32>,
    channels: Vec<u32>,
    zs: Vec<u32>,
    ts: Vec<u32>,
    members: HashMap<(usize, usize, usize, usize), usize>,
}

impl NisGrid {
    fn of(members: &[(SequenceName, usize)]) -> Self {
        let distinct = |axis: SequenceAxis| {
            let mut v: Vec<u32> = members.iter().filter_map(|(s, _)| s.index(axis)).collect();
            v.sort_unstable();
            v.dedup();
            if v.is_empty() { vec![0] } else { v }
        };
        let mut grid = NisGrid {
            positions: distinct(SequenceAxis::Position),
            channels: distinct(SequenceAxis::Channel),
            zs: distinct(SequenceAxis::Z),
            ts: distinct(SequenceAxis::Time),
            members: HashMap::new(),
        };
        for (sn, file) in members {
            if let (Some(p), Some(c), Some(z), Some(t)) = (
                pos_of(&grid.positions, sn.index(SequenceAxis::Position)),
                pos_of(&grid.channels, sn.index(SequenceAxis::Channel)),
                pos_of(&grid.zs, sn.index(SequenceAxis::Z)),
                pos_of(&grid.ts, sn.index(SequenceAxis::Time)),
            ) {
                grid.members.insert((p, c, z, t), *file);
            }
        }
        if members.is_empty() {
            grid.members.insert((0, 0, 0, 0), 0);
        }
        grid
    }
}

/// Where index number `n` (0 when absent) is in `v`.
fn pos_of(v: &[u32], n: Option<u32>) -> Option<usize> {
    v.iter().position(|&x| x == n.unwrap_or(0))
}

impl TiffDataset {
    /// A NIS-Elements TIFF export: the files of the export (same prefix, same index tokens) are
    /// read as one data set — positions as images, channels, Z planes and time points from the
    /// tokens; a file whose name has no tokens is one image.
    pub(crate) fn build_nis(&mut self, page0: &Ifd, nis: &NisPage) -> Result<()> {
        let order = self.main().header.byte_order;
        let lay = PageLayout::from_ifd(page0, order)?;
        let pt = lay.pixel_type()?;
        let own_name = file_name_of(&self.path);
        let own = parse_sequence_name(&own_name);
        let members = match &own {
            Some(own) => self.add_nis_members(own, &own_name),
            None => Vec::new(),
        };
        let grid = NisGrid::of(&members);
        let (n_c, n_z, n_t) = (grid.channels.len(), grid.zs.len(), grid.ts.len());
        check_plane_count((grid.positions.len() * n_c * n_z * n_t) as u64)?;
        let px = nis.pixel_size_um.or(resolution_um(page0).0);
        let py = nis.pixel_size_um.or(resolution_um(page0).1);
        let own_pi = match &own {
            Some(o) => pos_of(&grid.positions, o.index(SequenceAxis::Position)),
            None => Some(0),
        };
        let mut missing = 0usize;
        for (pi, &p) in grid.positions.iter().enumerate() {
            let mut info = ImageInfo::new(pi as u32, lay.width, lay.height, pt);
            info.size_c = n_c as u32;
            info.size_z = n_z as u32;
            info.size_t = n_t as u32;
            info.samples_per_pixel = if lay.planar == 2 {
                1
            } else {
                u32::from(lay.samples_per_pixel)
            };
            info.physical_size = PhysicalSize::micrometres(px, py, None);
            info.name = match &own {
                Some(o) if o.index(SequenceAxis::Position).is_some() => {
                    Some(format!("{}xy{p:02}", o.prefix))
                }
                Some(o) => Some(o.prefix.clone()),
                None => None,
            };
            let named_channels = own
                .as_ref()
                .and_then(|o| o.index(SequenceAxis::Channel))
                .is_some();
            info.channels = grid
                .channels
                .iter()
                .enumerate()
                .map(|(ci, &c)| ChannelInfo {
                    index: ci as u32,
                    name: named_channels.then(|| format!("c{c}")),
                    ..ChannelInfo::default()
                })
                .collect();
            info.instrument = Some(InstrumentInfo {
                manufacturer: Some("Nikon".into()),
                software: Some("NIS-Elements".into()),
                ..InstrumentInfo::default()
            });
            let mut planes = vec![None; n_c * n_z * n_t];
            for t in 0..n_t {
                for z in 0..n_z {
                    for c in 0..n_c {
                        let slot = (t * n_z + z) * n_c + c;
                        if let Some(&file) = grid.members.get(&(pi, c, z, t)) {
                            planes[slot] = Some(PlaneSrc::Page {
                                file,
                                page: 0,
                                sample: None,
                            });
                        } else {
                            missing += 1;
                        }
                    }
                }
            }
            if own_pi == Some(pi) {
                own_page_extra(&mut info, nis, &own_name);
            }
            self.series.push(Series {
                info: info.finish(),
                planes,
                levels: Vec::new(),
            });
        }
        if members.len() > 1 {
            self.notes.push(format!(
                "NIS-Elements TIFF export: {} files named '{}' + index tokens read as one data set ({} position(s) as images, {n_c} channel(s), {n_z} Z, {n_t} time point(s))",
                members.len(),
                own.as_ref().map_or("", |o| o.prefix.as_str()),
                grid.positions.len()
            ));
        }
        if missing > 0 {
            self.notes.push(format!(
                "{missing} (position, c, z, t) combination(s) have no file; reads of them fail (see `check`)"
            ));
        }
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::Inferred),
            ("images[].size_c", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::Inferred),
            ("images[].name", Source::Inferred),
            ("images[].channels[].name", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].extra.stage_position_um", Source::Inferred),
            ("images[].extra.nis_elements", Source::Inferred),
        ]);
        Ok(())
    }

    /// The opened file (member 0) and the files beside it with the same prefix, extension
    /// and index axes, registered as members (sorted by name).
    fn add_nis_members(
        &mut self,
        own: &SequenceName,
        own_name: &str,
    ) -> Vec<(SequenceName, usize)> {
        let mut members = vec![(own.clone(), 0)];
        let dir = dir_of(&self.path);
        let mut siblings: Vec<(String, PathBuf)> = self
            .fs
            .read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.metadata().is_ok_and(|m| m.is_file()))
                    .map(|e| e.path())
                    .filter(|p| file_name_of(p) != own_name)
                    .map(|p| (file_name_of(&p), p))
                    .collect()
            })
            .unwrap_or_default();
        siblings.sort();
        for (name, path) in siblings {
            if let Some(sn) = parse_sequence_name(&name)
                && sn.prefix == own.prefix
                && sn.extension == own.extension
                && sn.axes() == own.axes()
                && members.len() < MAX_NIS_FILES
            {
                self.files.push(FileSetMember::pending(path, name, None));
                members.push((sn, self.files.len() - 1));
            }
        }
        members
    }
}

/// The opened file's own private-tag values (time, stage position) on its image.
fn own_page_extra(info: &mut ImageInfo, nis: &NisPage, own_name: &str) {
    let mut m = Map::new();
    m.insert("file".into(), json!(own_name));
    if let Some(t) = nis.time_ms {
        m.insert("time_ms".into(), json!(t));
    }
    if let Some([x, y, z]) = nis.stage_um {
        info.extra
            .insert("stage_position_um".into(), json!({"x": x, "y": y, "z": z}));
    }
    info.extra.insert("nis_elements".into(), Value::Object(m));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let n = parse_sequence_name("single_plane_1-2000_2xy01c1.tif").unwrap();
        assert_eq!(n.prefix, "single_plane_1-2000_2");
        assert_eq!(
            n.indices,
            vec![(SequenceAxis::Position, 1), (SequenceAxis::Channel, 1)]
        );
        let n = parse_sequence_name("h4k8acxy22c1.tif").unwrap();
        assert_eq!(n.prefix, "h4k8ac");
        assert_eq!(n.index(SequenceAxis::Position), Some(22));
        let n = parse_sequence_name("h4k20me002xy13c2.TIF").unwrap();
        assert_eq!(
            (n.prefix.as_str(), n.extension.as_str()),
            ("h4k20me002", "tif")
        );
        let n = parse_sequence_name("run_t003xy2z05c1.tif").unwrap();
        assert_eq!(n.prefix, "run_");
        assert_eq!(n.index(SequenceAxis::Time), Some(3));
        assert_eq!(n.index(SequenceAxis::Z), Some(5));
        assert!(parse_sequence_name("plain.tif").is_none());
        assert!(parse_sequence_name("c1.tif").is_none());
        assert!(parse_sequence_name("a_c1c2.tif").is_none());
    }
}
