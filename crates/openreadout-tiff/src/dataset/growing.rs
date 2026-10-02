//! Growing-file evidence for a TIFF that is still being written.

use openreadout_core::reader::PlaneIndex;

use crate::dataset::{Flavor, PlaneSrc, TiffDataset};
use crate::tags;

impl TiffDataset {
    /// Growing-file evidence (`Dataset::write_state`) for a single-file TIFF whose IFD chain is
    /// intact: an `.ome.tif(f)` whose first page has no OME-XML yet (writers append it and
    /// patch the first page's description when the acquisition ends), OME-XML that declares
    /// planes the chain does not reach yet, or a last page whose strips run past the end (its
    /// data is being written). Pointers past the end of the file (a next-IFD offset, an
    /// out-of-line description) mean a finished file cut short and give `None`.
    pub(crate) fn growing_state(&self) -> Option<openreadout_core::live::WriteState> {
        if self.files.len() != 1 {
            return None;
        }
        let t = self.opened(0)?;
        if !t.problems.is_empty() || t.ifds.is_empty() {
            return None;
        }
        let file_len = t.file_len;
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let ome_name = [".ome.tif", ".ome.tiff", ".ome.tf2", ".ome.tf8", ".ome.btf"]
            .iter()
            .any(|e| name.ends_with(e));
        let page0_desc_empty = t.ifds[0].text(tags::IMAGE_DESCRIPTION).is_none_or(|d| {
            d.trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .is_empty()
        });
        let mut ws = openreadout_core::live::WriteState::new();
        let placeholder = ome_name && self.flavor == Flavor::Plain && page0_desc_empty;
        if placeholder {
            ws.missing.push("OME-XML".into());
            ws.evidence.push(
                "an .ome.tif file whose first page has no OME-XML yet (written when the acquisition ends)".into(),
            );
        }
        let mut declared = 0u64;
        let mut missing_declared = false;
        let mut done: Vec<(usize, u32, PlaneIndex)> = Vec::new();
        let mut end_complete = 0u64;
        let mut units = Vec::new();
        let mut partial_last = false;
        let last_page = t.ifds.len() - 1;
        for (image, se) in self.series.iter().enumerate() {
            let (nc, nz) = (se.info.size_c.max(1), se.info.size_z.max(1));
            for (slot, src) in se.planes.iter().enumerate() {
                declared += 1;
                let Some(PlaneSrc::Page { file: 0, page, .. }) = src else {
                    missing_declared |= src.is_none();
                    continue;
                };
                let Ok(l) = self.layout(0, *page) else {
                    continue;
                };
                let end = l
                    .offsets
                    .iter()
                    .zip(&l.byte_counts)
                    .map(|(o, n)| o.saturating_add(*n))
                    .max()
                    .unwrap_or(0);
                if end > file_len {
                    if *page == last_page {
                        partial_last = true;
                    } else {
                        return None; // data missing before the last page: damage, not growth
                    }
                    continue;
                }
                let ifd_end = t.ifds[*page]
                    .offset
                    .saturating_add(16 + 20 * t.ifds[*page].fields.len() as u64);
                end_complete = end_complete.max(end).max(ifd_end.min(file_len));
                units.push(l.byte_counts.iter().sum::<u64>());
                let slot = slot as u32;
                let idx = PlaneIndex {
                    c: slot % nc,
                    z: (slot / nc) % nz,
                    t: slot / (nc * nz),
                };
                done.push((*page, image as u32, idx));
            }
        }
        let declared_gap = missing_declared && self.flavor == Flavor::OmeTiff;
        if declared_gap {
            ws.missing.push("pages".into());
            ws.evidence
                .push("the OME-XML declares planes the IFD chain does not reach yet".into());
            ws.expected_planes = Some(declared);
        }
        if partial_last {
            ws.evidence
                .push("the last page's pixel data is still being written".into());
        }
        if !(placeholder || partial_last || declared_gap) {
            return None;
        }
        done.sort_by_key(|(page, _, _)| *page);
        ws.complete = done.into_iter().map(|(_, i, p)| (i, p)).collect();
        ws.tail_bytes = file_len.saturating_sub(end_complete);
        units.sort_unstable();
        ws.unit_bytes = units.get(units.len() / 2).copied();
        Some(ws)
    }
}
