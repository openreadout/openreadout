//! `Dataset` for an iSyntax slide: one RGB pyramid, the label and macro images and the ICC
//! profiles as attachments.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{Region, ResolutionLevel};
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Value, json};

use super::header::{self, Header};
use super::pyramid::{Pyramid, SEEK_ENTRY, parse_seek_table};
use super::render::{Decoder, TileKey};
use super::{FORMAT_ID, IsyntaxReader};

/// Bytes read at a time while looking for the end of the header.
const HEADER_CHUNK: usize = 1 << 20;
/// Largest header accepted.
const MAX_HEADER: u64 = 1 << 30;
/// Largest seek table read.
const MAX_SEEK_TABLE: u64 = 1 << 31;
/// Tile rows composed at a time.
const BAND_TILES: u64 = 4;

/// An opened iSyntax slide.
pub struct IsyntaxDataset {
    path: PathBuf,
    file: SourceFile,
    len: u64,
    header: Header,
    pyramid: Arc<Pyramid>,
    /// Offset and size of the seek table's entries (after its 8-byte tag).
    seek_table: (u64, u64),
    decoder: Option<Decoder>,
}

impl std::fmt::Debug for IsyntaxDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IsyntaxDataset")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// `YYYYMMDDhhmmss[.ffffff]` → ISO-8601 without a zone.
pub fn dicom_datetime(v: &str) -> Option<String> {
    let v = v.trim();
    let d = v.get(..14)?;
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut s = format!(
        "{}-{}-{}T{}:{}:{}",
        &d[..4],
        &d[4..6],
        &d[6..8],
        &d[8..10],
        &d[10..12],
        &d[12..14]
    );
    if let Some(frac) = v.get(14..).and_then(|f| f.strip_prefix('.'))
        && !frac.is_empty()
        && frac.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().any(|b| b != b'0')
    {
        s.push('.');
        s.push_str(frac);
    }
    Some(s)
}

enum AttachmentSource {
    Image(usize),
    Icc(usize),
}

impl IsyntaxDataset {
    /// Open the slide named by `input`.
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let file = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = file.size().map_err(|e| Error::io(path, e))?;
        // The header ends at the first 0x04.
        let mut head = Vec::new();
        let eot = loop {
            let at = head.len() as u64;
            if at >= len.min(MAX_HEADER) {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    "no end-of-header byte (0x04): the XML header is cut short",
                ));
            }
            let n = (len - at).min(HEADER_CHUNK as u64) as usize;
            let mut buf = vec![0u8; n];
            file.read_exact_at(at, &mut buf)
                .map_err(|e| Error::io(path, e))?;
            if let Some(i) = buf.iter().position(|&b| b == 0x04) {
                head.extend_from_slice(&buf[..i]);
                break at + i as u64;
            }
            head.extend_from_slice(&buf);
        };
        let header = header::parse(&head)?;
        drop(head);
        let wsi = header
            .image("WSI")
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "the header has no WSI scanned image"))?;
        let pyramid = Pyramid::from_header(&header, wsi)?;
        // The seek table: tag (0x301D, 0x2015) and its size, then 40-byte entries.
        let mut tag = [0u8; 8];
        file.read_exact_at(eot + 1, &mut tag).map_err(|_| {
            Error::corrupt_at(FORMAT_ID, eot + 1, "the file ends before the seek table")
        })?;
        let group = u16::from_le_bytes([tag[0], tag[1]]);
        let element = u16::from_le_bytes([tag[2], tag[3]]);
        if (group, element) != (0x301D, 0x2015) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                eot + 1,
                format!(
                    "expected the seek table (0x301D,0x2015) after the header, found ({group:#06x},{element:#06x})"
                ),
            ));
        }
        let declared = u64::from(u32::from_le_bytes([tag[4], tag[5], tag[6], tag[7]]));
        let size = if declared == u64::from(u32::MAX) {
            // Undefined length: as many entries as the largest block id needs.
            pyramid
                .blocks
                .iter()
                .map(|b| b.block_id + 1)
                .max()
                .unwrap_or(0)
                * SEEK_ENTRY
        } else {
            declared
        };
        let start = eot + 9;
        if size > MAX_SEEK_TABLE || start.saturating_add(size) > len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                eot + 1,
                format!("seek table of {size} bytes runs past the end of the file ({len} bytes)"),
            ));
        }
        Ok(IsyntaxDataset {
            path: path.to_path_buf(),
            file,
            len,
            header,
            pyramid: Arc::new(pyramid),
            seek_table: (start, size),
            decoder: None,
        })
    }

    fn block_locations(&self) -> Result<Vec<(u64, u64)>> {
        let (start, size) = self.seek_table;
        let mut table = vec![0u8; size as usize];
        self.file
            .read_exact_at(start, &mut table)
            .map_err(|e| Error::io(&self.path, e))?;
        parse_seek_table(&table, &self.pyramid, self.len, start + size)
    }

    fn decoder(&mut self) -> Result<&mut Decoder> {
        if self.decoder.is_none() {
            let blocks = self.block_locations()?;
            self.decoder = Some(Decoder::new(
                self.pyramid.clone(),
                self.file.clone(),
                blocks,
            ));
        }
        self.decoder
            .as_mut()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "decoder unavailable"))
    }

    fn wsi(&self) -> Option<&header::ScannedImage> {
        self.header.image("WSI")
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self) -> ImageInfo {
        let p = &self.pyramid;
        let (w, h) = p.level_size(0);
        let (w32, h32) = (
            u32::try_from(w).unwrap_or(u32::MAX),
            u32::try_from(h).unwrap_or(u32::MAX),
        );
        let mut info = ImageInfo::new(0, w32, h32, PixelType::Uint8);
        info.samples_per_pixel = 3;
        info.dimension_order = "XYCZT".into();
        info.pyramid_levels = p.levels;
        let (tw, th) = p.tile_size();
        info.resolution_levels = (0..p.levels)
            .map(|s| {
                let (lw, lh) = p.level_size(s);
                let mut r = ResolutionLevel::new(
                    s,
                    u32::try_from(lw).unwrap_or(u32::MAX),
                    u32::try_from(lh).unwrap_or(u32::MAX),
                    w32,
                    h32,
                )
                .with_tile(tw as u32, th as u32);
                let f = f64::from(1u32 << s);
                r.downsample_x = f;
                r.downsample_y = f;
                r
            })
            .collect();
        info.physical_size = PhysicalSize::micrometres(p.mpp.0, p.mpp.1, None);
        info.channels = vec![ChannelInfo {
            index: 0,
            acquisition_mode: Some("Brightfield (RGB)".into()),
            ..ChannelInfo::default()
        }];
        let h = &self.header;
        let versions = h
            .attribute("DICOM_SOFTWARE_VERSIONS")
            .map(header::string_array)
            .unwrap_or_default();
        info.instrument = Some(InstrumentInfo {
            manufacturer: h.attribute("DICOM_MANUFACTURER").map(str::to_string),
            model: h
                .attribute("DICOM_MANUFACTURERS_MODEL_NAME")
                .map(str::to_string),
            software: None,
            software_version: versions.first().cloned(),
            detector: None,
        });
        info.acquired_at = h
            .attribute("DICOM_ACQUISITION_DATETIME")
            .and_then(dicom_datetime);
        let mut m = serde_json::Map::new();
        m.insert(
            "data_model".into(),
            json!(h.attribute("PIM_DP_UFS_INTERFACE_VERSION")),
        );
        m.insert("compression_method".into(), json!(p.compression));
        if let Some(wsi) = self.wsi() {
            for (k, attr) in [
                ("derivation_description", "DICOM_DERIVATION_DESCRIPTION"),
                ("lossy_compression", "DICOM_LOSSY_IMAGE_COMPRESSION"),
                (
                    "pixel_transformation_method",
                    "UFS_IMAGE_PIXEL_TRANSFORMATION_METHOD",
                ),
            ] {
                if let Some(v) = wsi.attributes.get(attr) {
                    m.insert(k.into(), json!(v));
                }
            }
            for (k, attr) in [
                (
                    "lossy_compression_ratio",
                    "DICOM_LOSSY_IMAGE_COMPRESSION_RATIO",
                ),
                (
                    "lossy_compression_method",
                    "DICOM_LOSSY_IMAGE_COMPRESSION_METHOD",
                ),
            ] {
                if let Some(v) = wsi.attributes.get(attr) {
                    m.insert(k.into(), json!(header::string_array(v).join(" ")));
                }
            }
            m.insert("declared_blocks".into(), json!(wsi.number_of_blocks));
        }
        m.insert("code_blocks".into(), json!(p.blocks.len()));
        m.insert("block_size".into(), json!([p.block_w, p.block_h]));
        m.insert("scales".into(), json!(p.levels));
        m.insert("padded_size".into(), json!([p.padded.0, p.padded.1]));
        m.insert("padding".into(), json!(p.padding));
        m.insert("image_offset".into(), json!([p.offset.0, p.offset.1]));
        m.insert(
            "tiles_per_scale".into(),
            json!(
                (0..p.levels)
                    .map(|s| p.existing_tiles(s))
                    .collect::<Vec<_>>()
            ),
        );
        m.insert("software_versions".into(), json!(versions));
        if let Some(s) = h.attribute("DICOM_DEVICE_SERIAL_NUMBER") {
            m.insert("scanner_serial".into(), json!(s));
        }
        for (k, attr) in [
            ("calibration_dates", "DICOM_DATE_OF_LAST_CALIBRATION"),
            ("calibration_times", "DICOM_TIME_OF_LAST_CALIBRATION"),
        ] {
            if let Some(v) = h.attribute(attr) {
                m.insert(k.into(), json!(header::string_array(v)));
            }
        }
        if let Some(s) = h.attribute("PIIM_DP_SCANNER_CALIBRATION_STATUS") {
            m.insert("calibration_status".into(), json!(s));
        }
        if let Some(b) = header::barcode(h) {
            m.insert("barcode".into(), json!(b));
        }
        info.extra.insert("isyntax".into(), Value::Object(m));
        info.extra.insert(
            "codec".into(),
            json!(format!("isyntax wavelet (method {})", p.compression)),
        );
        info.finish()
    }

    fn attachment_list(&self) -> Vec<(AttachmentInfo, AttachmentSource)> {
        let mut out = Vec::new();
        let mut push = |name: String,
                        ct: &str,
                        ext: &str,
                        size: usize,
                        src: AttachmentSource,
                        extra: BTreeMap<String, Value>| {
            out.push((
                AttachmentInfo {
                    index: out.len() as u32,
                    name,
                    content_type: ct.into(),
                    extension: ext.into(),
                    offset: None,
                    size: size as u64,
                    extra,
                },
                src,
            ));
        };
        for (k, im) in self.header.images.iter().enumerate() {
            let role = match im.kind.as_str() {
                "LABELIMAGE" => "label",
                "MACROIMAGE" => "macro",
                _ => "",
            };
            if let Some(d) = &im.image_data
                && !role.is_empty()
            {
                let mut extra = BTreeMap::new();
                extra.insert("image_type".into(), json!(im.kind));
                if let Some((w, h)) = crate::image::header_size(d) {
                    extra.insert("width".into(), json!(w));
                    extra.insert("height".into(), json!(h));
                }
                push(
                    role.into(),
                    "JPG",
                    "jpg",
                    d.len(),
                    AttachmentSource::Image(k),
                    extra,
                );
            }
        }
        for (k, im) in self.header.images.iter().enumerate() {
            if let Some(icc) = &im.icc_profile {
                let mut extra = BTreeMap::new();
                extra.insert("image_type".into(), json!(im.kind));
                push(
                    format!("icc profile ({})", im.kind.to_ascii_lowercase()),
                    "ICC",
                    "icc",
                    icc.len(),
                    AttachmentSource::Icc(k),
                    extra,
                );
            }
        }
        out
    }

    #[allow(clippy::many_single_char_names)]
    fn read_level_region(
        &mut self,
        index: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        if index.c != 0 || index.z != 0 || index.t != 0 {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} is out of range (C=1, Z=1, T=1)",
                index.c, index.z, index.t
            )));
        }
        let p = self.pyramid.clone();
        if level >= p.levels {
            return Err(Error::Usage(format!(
                "pyramid level {level} out of range (0..{})",
                p.levels
            )));
        }
        let (lw, lh) = p.level_size(level);
        let (lw, lh) = (
            u32::try_from(lw).unwrap_or(u32::MAX),
            u32::try_from(lh).unwrap_or(u32::MAX),
        );
        let win = match region {
            Some(r) => {
                r.check_within(lw, lh, &format!("image 0 level {level}"))?;
                r
            }
            None => Region::full(lw, lh),
        };
        let n = openreadout_core::pixel::plane_bytes_checked(FORMAT_ID, win.width, win.height, 3)
            .map_err(|_| {
                Error::unsupported(
                    FORMAT_ID,
                    format!("a {} x {} plane in memory", win.width, win.height),
                    format!(
                        "Read a region of it (`--region X,Y,W,H`) or a downsampled pyramid level (`--level N`, N up to {}); level sizes are in `info` → images[].resolution_levels.",
                        p.levels - 1
                    ),
                )
            })?;
        let mut data = vec![255u8; n];
        if win.width == 0 || win.height == 0 {
            return Ok(Plane {
                width: win.width,
                height: win.height,
                pixel_type: PixelType::Uint8,
                samples_per_pixel: 3,
                data,
            });
        }
        let (tw, th) = p.tile_size();
        let origin = p.level_origin(level);
        let (cols, rows) = p.tiles(level);
        let gx0 = u64::from(win.x) + origin;
        let gy0 = u64::from(win.y) + origin;
        let gx1 = gx0 + u64::from(win.width); // exclusive
        let gy1 = gy0 + u64::from(win.height);
        let (tx0, tx1) = (gx0 / tw, (gx1 - 1) / tw);
        let (ty0, ty1) = (gy0 / th, (gy1 - 1) / th);
        let row_bytes = win.width as usize * 3;
        let mut band = ty0;
        while band <= ty1 {
            let band_end = (band + BAND_TILES - 1).min(ty1);
            let want: Vec<TileKey> = (band..=band_end)
                .flat_map(|ty| (tx0..=tx1).map(move |tx| (level, tx as i64, ty as i64)))
                .filter(|k| (k.1 as u64) < cols && (k.2 as u64) < rows && p.exists(k.0, k.1, k.2))
                .collect();
            let tiles = self.decoder()?.tiles(level, &want)?;
            for (k, px) in &tiles {
                let (tx, ty) = (k.1 as u64, k.2 as u64);
                // Overlap of the tile with the window, in grid pixels.
                let x0 = (tx * tw).max(gx0);
                let x1 = ((tx + 1) * tw).min(gx1);
                let y0 = (ty * th).max(gy0);
                let y1 = ((ty + 1) * th).min(gy1);
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let cw = ((x1 - x0) * 3) as usize;
                for gy in y0..y1 {
                    let src = (((gy - ty * th) * tw + (x0 - tx * tw)) * 3) as usize;
                    let dst = (gy - gy0) as usize * row_bytes + ((x0 - gx0) * 3) as usize;
                    data[dst..dst + cw].copy_from_slice(&px[src..src + cw]);
                }
            }
            band = band_end + 1;
        }
        Ok(Plane {
            width: win.width,
            height: win.height,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: 3,
            data,
        })
    }

    fn check_impl(&mut self, full: bool) -> CheckReport {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("XML header: data objects, scanned images, block header templates, general header ranges");
        r.performed("block header table: 48-byte entries, every code block on the tile grid of its scale, no duplicates");
        r.performed("seek table: tag and size inside the file");
        let p = self.pyramid.clone();
        for s in 0..p.levels {
            if p.existing_tiles(s) == 0 {
                r.push(Finding::warning(
                    "empty_scale",
                    format!("scale {s} has no code blocks"),
                ));
            }
        }
        r.performed("seek table: an entry for every code block, each block inside the data");
        if let Err(e) = self.block_locations() {
            r.push(Finding::error("bad_seek_table", e.to_string()));
            return r;
        }
        if full {
            r.performed("the tiles of the top scale decode (code blocks, inverse transform)");
            let top = p.levels - 1;
            let (cols, rows) = p.tiles(top);
            let want: Vec<TileKey> = (0..rows as i64)
                .flat_map(|y| (0..cols as i64).map(move |x| (top, x, y)))
                .filter(|k| p.exists(k.0, k.1, k.2))
                .collect();
            match self.decoder().and_then(|d| d.tiles(top, &want)) {
                Ok(_) => {}
                Err(e) => r.push(Finding::error("bad_block", e.to_string())),
            }
        }
        r
    }
}

impl Dataset for IsyntaxDataset {
    fn info(&self) -> Result<FileInfo> {
        let image = self.image_info();
        let plane_count = image.plane_count;
        let notes = vec![
            "an iSyntax slide: a wavelet pyramid decoded tile by tile; read a region with `--region X,Y,W,H` or a coarser level with `--level N` (level 0 of a whole slide is too large to read whole)".to_string(),
            "pixels are the decoded image converted from YCoCg to RGB; the post-processing and ICC profile stored in the header are not applied (the profiles are attachments)".to_string(),
            "tiles the file does not store (background) are white".to_string(),
        ];
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.len,
            format: IsyntaxReader.descriptor(),
            format_version: self
                .header
                .attribute("PIM_DP_UFS_INTERFACE_VERSION")
                .map(str::to_string),
            images: vec![image],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({ "header": self.header.tree }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].pixel_type",
            "images[].samples_per_pixel",
            "images[].pyramid_levels",
            "images[].resolution_levels",
            "images[].physical_size",
            "images[].extra.isyntax",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        for k in ["images[].instrument", "images[].acquired_at"] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    #[allow(clippy::many_single_char_names)]
    fn entries(&self) -> Result<Vec<LsEntry>> {
        let p = &self.pyramid;
        let mut out = vec![
            LsEntry {
                kind: "metadata".into(),
                name: "XML header".into(),
                offset: Some(0),
                size: Some(self.header.length),
                image: None,
                details: json!({"scanned_images": self.header.images.iter().map(|i| i.kind.clone()).collect::<Vec<_>>()}),
            },
            LsEntry {
                kind: "index".into(),
                name: "seek table".into(),
                offset: Some(self.seek_table.0),
                size: Some(self.seek_table.1),
                image: None,
                details: json!({"entries": self.seek_table.1 / SEEK_ENTRY}),
            },
        ];
        for s in 0..p.levels {
            let (w, h) = p.level_size(s);
            let (c, r) = p.tiles(s);
            out.push(LsEntry {
                kind: "pyramid-level".into(),
                name: format!("level {s}"),
                offset: None,
                size: None,
                image: Some(0),
                details: json!({"level": s, "size_x": w, "size_y": h, "tile_grid": [c, r], "tiles_stored": p.existing_tiles(s)}),
            });
        }
        for (a, _) in self.attachment_list() {
            out.push(LsEntry {
                kind: "attachment".into(),
                name: a.name.clone(),
                offset: None,
                size: Some(a.size),
                image: None,
                details: json!({"content_type": a.content_type}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.read_plane_level(image, index, 0)
    }

    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image {image} does not exist (1 image)"
            )));
        }
        self.read_level_region(index, level, None)
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image {image} does not exist (1 image)"
            )));
        }
        self.read_level_region(index, level, Some(region))
    }

    fn check(&mut self) -> Result<CheckReport> {
        Ok(self.check_impl(true))
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        Ok(self.check_impl(false))
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self.attachment_list().into_iter().map(|(a, _)| a).collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let list = self.attachment_list();
        let (_, src) = list.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "attachment {index} does not exist ({} attachments)",
                list.len()
            ))
        })?;
        let im = |k: &usize| &self.header.images[*k];
        Ok(match src {
            AttachmentSource::Image(k) => im(k).image_data.clone().unwrap_or_default(),
            AttachmentSource::Icc(k) => im(k).icc_profile.clone().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dicom_datetimes() {
        assert_eq!(
            dicom_datetime("20210609111602.000000").as_deref(),
            Some("2021-06-09T11:16:02")
        );
        assert_eq!(
            dicom_datetime("20210609111602.5").as_deref(),
            Some("2021-06-09T11:16:02.5")
        );
        assert_eq!(dicom_datetime("2021-06-09"), None);
    }
}
