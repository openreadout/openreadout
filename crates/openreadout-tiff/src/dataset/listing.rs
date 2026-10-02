//! `ls`: the structure of a TIFF data set (header, metadata blocks, pages, pyramid levels and
//! member files).

use std::collections::HashMap;

use openreadout_core::model::LsEntry;
use serde_json::json;

use crate::container::ByteOrder;
use crate::dataset::{PlaneSrc, TiffDataset};
use crate::decode::PageLayout;
use crate::files::file_name_of;
use crate::tags;

impl TiffDataset {
    pub(crate) fn list_entries(&self) -> Vec<LsEntry> {
        let mut out = self.metadata_entries();
        out.extend(self.page_entries());
        out.extend(self.level_entries());
        out.extend(self.member_entries());
        out
    }

    /// The TIFF header, the `.nd` file and the OME-XML.
    fn metadata_entries(&self) -> Vec<LsEntry> {
        let mut out = Vec::new();
        if let Some(t) = self.opened(0) {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "tiff-header".into(),
                offset: Some(0),
                size: Some(if t.header.big_tiff { 16 } else { 8 }),
                image: None,
                details: json!({
                    "byte_order": if t.header.byte_order == ByteOrder::Little { "II" } else { "MM" },
                    "big_tiff": t.header.big_tiff,
                    "first_ifd_offset": t.header.first_ifd_offset,
                    "sub_format": self.flavor.id(),
                }),
            });
        }
        if let Some(nd) = &self.nd {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "nd".into(),
                offset: Some(0),
                size: Some(self.file_len),
                image: None,
                details: json!({
                    "file": file_name_of(&self.path),
                    "version": nd.version,
                    "stages": nd.stages.len(),
                    "wavelengths": nd.waves.len(),
                    "time_points": nd.time_points,
                    "z_steps": nd.z_steps,
                }),
            });
        }
        if let Some((file, offset, xml)) = &self.ome_xml {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "ome-xml".into(),
                offset: *offset,
                size: Some(xml.len() as u64),
                image: None,
                details: json!({"file": self.files[*file].name}),
            });
        }
        out
    }

    /// Every page of the main file, with the image and role it has.
    fn page_entries(&self) -> Vec<LsEntry> {
        let Some(t) = self.opened(0) else {
            return Vec::new();
        };
        let mut owner: HashMap<(usize, usize), (u32, &'static str)> = HashMap::new();
        for (si, s) in self.series.iter().enumerate() {
            for p in s.planes.iter().flatten() {
                if let PlaneSrc::Page { file, page, .. } = p {
                    owner.entry((*file, *page)).or_insert((si as u32, "plane"));
                }
            }
            for l in &s.levels {
                if let Some(p) = l.page {
                    owner.entry((0, p)).or_insert((si as u32, "pyramid-level"));
                }
            }
        }
        t.ifds
            .iter()
            .enumerate()
            .map(|(i, ifd)| {
                let l = PageLayout::from_ifd(ifd, t.header.byte_order).ok();
                let att = self.attachments.iter().find(|a| a.page == i);
                let (image, role) = match (owner.get(&(0, i)), att) {
                    (Some((im, r)), _) => (Some(*im), (*r).to_string()),
                    (None, Some(a)) => (None, a.role.clone()),
                    (None, None) => (None, "unused".to_string()),
                };
                LsEntry {
                    kind: if role == "plane" || role == "pyramid-level" || role == "unused" {
                        "page".into()
                    } else {
                        "attachment".into()
                    },
                    name: format!("page {i}"),
                    offset: Some(ifd.offset),
                    size: l.as_ref().map(|l| l.byte_counts.iter().sum()),
                    image,
                    details: json!({
                        "role": role,
                        "width": l.as_ref().map(|l| l.width),
                        "height": l.as_ref().map(|l| l.height),
                        "samples_per_pixel": l.as_ref().map(|l| l.samples_per_pixel),
                        "bits_per_sample": l.as_ref().map(|l| l.bits_per_sample),
                        "compression": l.as_ref().map(PageLayout::compression_name),
                        "tiled": l.as_ref().map(|l| l.tiled),
                        "chunks": l.as_ref().map(|l| l.offsets.len()),
                        "sub_ifds": ifd.uints(tags::SUB_IFDS).map(|v| v.len()),
                    }),
                }
            })
            .collect()
    }

    /// The reduced-resolution levels of every image.
    fn level_entries(&self) -> Vec<LsEntry> {
        let mut out = Vec::new();
        for (si, s) in self.series.iter().enumerate() {
            for (li, l) in s.levels.iter().enumerate() {
                let page_offset = l
                    .page
                    .and_then(|p| self.opened(0).and_then(|t| t.ifds.get(p)).map(|i| i.offset));
                out.push(LsEntry {
                    kind: "pyramid-level".into(),
                    name: format!("image {si} level {}", li + 1),
                    offset: l.sub_ifd.or(page_offset),
                    size: None,
                    image: Some(si as u32),
                    details: json!({"width": l.width, "height": l.height, "page": l.page, "sub_ifd": l.sub_ifd.is_some()}),
                });
            }
        }
        out
    }

    /// The other files of the set (OME-TIFF members, companion XML, `.nd` and NIS-Elements
    /// series files).
    fn member_entries(&self) -> Vec<LsEntry> {
        self.files
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, f)| LsEntry {
                kind: "file".into(),
                name: f.name.clone(),
                offset: None,
                size: self.fs.metadata(&f.path).ok().map(|m| m.len()),
                image: None,
                details: json!({
                    "role": if f.metadata_only { "metadata" } else { "pixels" },
                    "exists": self.fs.exists(&f.path),
                    "uuid": f.uuid,
                    "member": i,
                }),
            })
            .collect()
    }
}
