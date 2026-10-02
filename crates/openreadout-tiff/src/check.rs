//! `check`: structural validation of the IFD chain, strip/tile extents and the OME plane map.
//! See `docs/formats/tiff.md` § Integrity checks.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use openreadout_core::model::{CheckReport, Finding};
use openreadout_core::{Error, Result};
use serde_json::Value;

use crate::FORMAT_ID;
use crate::container::TiffFile;
use crate::dataset::{Flavor, PlaneSrc, TiffDataset};
use crate::decode::PageLayout;
use crate::eer::EerFrameEvents;

/// Cap per finding code so a damaged 100 000-tile slide does not print 100 000 lines.
const MAX_PER_CODE: usize = 20;

struct Report {
    r: CheckReport,
    counts: BTreeMap<&'static str, usize>,
}

impl Report {
    fn push(&mut self, code: &'static str, f: Finding) {
        let n = self.counts.entry(code).or_insert(0);
        *n += 1;
        if *n <= MAX_PER_CODE {
            self.r.push(f);
        } else if *n == MAX_PER_CODE + 1 {
            let sev_err = f.severity == openreadout_core::model::Severity::Error;
            let msg = format!("further '{code}' findings suppressed");
            self.r.push(if sev_err {
                Finding::error(code, msg)
            } else {
                Finding::warning(code, msg)
            });
        }
    }
}

/// EER: decode every frame's events and compare the count with the frame's recorded dose
/// (electrons per pixel, which the writer stores as events / pixels to 6 decimals).
fn eer_events(parts: &mut CheckParts<'_>, rep: &mut Report) {
    rep.r.performed(
        "EER: every frame's event stream decodes within its strip, and its event count matches the frame's recorded dose (events / pixels to 6 decimals)",
    );
    let (mut frames, mut compared, mut total, mut frame_pixels) = (0u64, 0u64, 0u64, 0u64);
    for (page, r) in parts.eer_event_counts() {
        frames += 1;
        match r {
            Err(e) => rep.push(
                "eer_bad_frame",
                Finding::error("eer_bad_frame", format!("frame page {page}: {e}")),
            ),
            Ok((events, pixels, dose)) => {
                total += events;
                frame_pixels = pixels;
                if let Some(d) = dose
                    && pixels > 0
                {
                    compared += 1;
                    let measured = events as f64 / pixels as f64;
                    if (measured - d).abs() > 1.0e-6 {
                        rep.push(
                            "eer_dose_mismatch",
                            Finding::warning(
                                "eer_dose_mismatch",
                                format!(
                                    "frame page {page}: {events} events on {pixels} pixels = {measured:.6} e/pixel, the frame records a dose of {d}"
                                ),
                            ),
                        );
                    }
                }
            }
        }
    }
    let per_pixel = if frame_pixels > 0 {
        format!(" ({:.6} e/pixel", total as f64 / frame_pixels as f64)
            + &parts.eer_total_dose().map_or_else(
                || ")".to_string(),
                |d| format!("; the acquisition records totalDose {d})"),
            )
    } else {
        String::new()
    };
    rep.push(
        "eer_events",
        Finding::info(
            "eer_events",
            format!(
                "{total} electron events in {frames} frames{per_pixel}; {compared} frame(s) compared with their recorded dose"
            ),
        ),
    );
}

pub(crate) fn run(ds: &mut TiffDataset) -> CheckReport {
    let mut parts = ds.check_parts();
    let mut rep = Report {
        r: CheckReport::new(parts.path(), FORMAT_ID),
        counts: BTreeMap::new(),
    };
    rep.r
        .performed("TIFF/BigTIFF header: byte order, version 42/43, first IFD offset");
    rep.r
        .performed("IFD chain: every directory lies inside the file, no loops, chain terminates");
    rep.r
        .performed("every out-of-line field value lies inside the file");
    rep.r.performed(
        "every page: dimensions, sample layout, and strip/tile count match its geometry",
    );
    rep.r
        .performed("every strip/tile (offset + byte count) lies inside the file");
    rep.r
        .performed("every (c, z, t) plane of every image maps to an existing page");
    rep.r.performed(
        "every JPEG page: the photometric tag and the first tile's JFIF/Adobe markers form a colour coding the decoder handles",
    );
    if matches!(parts.flavor(), Flavor::OmeTiff | Flavor::OmeCompanion) {
        rep.r.performed(
            "OME TiffData: referenced files exist and are TIFFs, IFD indices exist, UUIDs agree",
        );
    }

    // Members referenced by the plane map, plus the opened file.
    let (refs, contiguous) = parts.plane_refs();
    let mut used_pages: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (_, _, _, _, r) in &refs {
        if let Some((f, p)) = r {
            used_pages.entry(*f).or_default().insert(*p);
        }
    }
    if !parts.member_is_metadata(0) {
        used_pages.entry(0).or_default();
    }
    let mut unreadable: BTreeSet<usize> = BTreeSet::new();
    for &file in used_pages.keys() {
        if let Err(e) = parts.open_member(file) {
            unreadable.insert(file);
            let code = if matches!(e, Error::Corrupt { .. }) && e.to_string().contains("missing") {
                "missing_file"
            } else {
                "unreadable_file"
            };
            rep.push(
                code,
                Finding::error(code, format!("'{}': {e}", parts.member_name(file))),
            );
        }
    }

    // Structure of every opened member: chain problems and every page's chunk extents.
    for &file in used_pages.keys() {
        if unreadable.contains(&file) {
            continue;
        }
        let Some(t) = parts.member_file(file) else {
            continue;
        };
        let name = parts.member_name(file);
        let prefix = if file == 0 {
            String::new()
        } else {
            format!("'{name}': ")
        };
        for p in &t.problems {
            let f = Finding::error(p.code, format!("{prefix}{}", p.detail)).at(p.offset);
            rep.push(p.code, f);
        }
        let file_len = t.file_len;
        let n_pages = t.ifds.len();
        for page in 0..n_pages {
            let layout = match parts.layout(file, page) {
                Ok(l) => l,
                Err(e) => {
                    let off = parts.member_file(file).map_or(0, |t| t.ifds[page].offset);
                    rep.push(
                        "bad_page",
                        Finding::error("bad_page", format!("{prefix}page {page}: {e}")).at(off),
                    );
                    continue;
                }
            };
            let need = layout.expected_chunks();
            let have = layout.offsets.len().min(layout.byte_counts.len()) as u64;
            if have < need {
                rep.push(
                    "missing_chunks",
                    Finding::error(
                        "missing_chunks",
                        format!(
                            "{prefix}page {page}: geometry needs {need} strips/tiles but the page lists {} offsets and {} byte counts",
                            layout.offsets.len(),
                            layout.byte_counts.len()
                        ),
                    ),
                );
            }
            let mut beyond = 0u64;
            let mut first_beyond = None;
            let mut sparse = 0u64;
            for (o, n) in layout.offsets.iter().zip(&layout.byte_counts) {
                if *o == 0 || *n == 0 {
                    sparse += 1;
                    continue;
                }
                if o.checked_add(*n).is_none_or(|e| e > file_len) {
                    beyond += 1;
                    first_beyond.get_or_insert(*o);
                }
            }
            if beyond > 0 {
                rep.push(
                    "truncated",
                    Finding::error(
                        "truncated",
                        format!(
                            "{prefix}page {page}: {beyond} of {} strips/tiles extend past the end of the file ({file_len} bytes); pixel data is missing",
                            layout.offsets.len()
                        ),
                    )
                    .at(first_beyond.unwrap_or(0)),
                );
            }
            if sparse > 0 {
                rep.push(
                    "sparse_chunks",
                    Finding::info(
                        "sparse_chunks",
                        format!("{prefix}page {page}: {sparse} strips/tiles were never written (read as zeros)"),
                    ),
                );
            }
            if let Err(e) = layout.pixel_type() {
                rep.push(
                    "unsupported_samples",
                    Finding::warning("unsupported_samples", format!("{prefix}page {page}: {e}")),
                );
            } else if layout.compression == 7 {
                // JPEG: can the samples be turned into what the page declares? Decided from the
                // tags and the first chunk's markers, as `read_plane` does (docs/formats/tiff.md
                // § JPEG colour).
                match parts.jpeg_decision(file, &layout) {
                    Ok(Some(d)) if d.conflict => rep.push(
                        "jpeg_colour_ambiguous",
                        Finding::warning(
                            "jpeg_colour_ambiguous",
                            format!("{prefix}page {page}: photometric RGB, but the JPEG stream's Adobe segment declares YCbCr; decoded as RGB (as tifffile/libtiff do), colours may be wrong"),
                        ),
                    ),
                    Ok(_) => {}
                    Err(e @ Error::Unsupported { .. }) => rep.push(
                        "unsupported_samples",
                        Finding::warning(
                            "unsupported_samples",
                            format!("{prefix}page {page}: {e}"),
                        ),
                    ),
                    Err(e) => rep.push(
                        "bad_chunk",
                        Finding::error("bad_chunk", format!("{prefix}page {page}: {e}")),
                    ),
                }
            } else if !crate::decode::is_decoded(layout.compression) {
                rep.push(
                    "unsupported_compression",
                    Finding::warning(
                        "unsupported_compression",
                        format!(
                            "{prefix}page {page}: compression {} ({}) is not decoded; the structure is valid",
                            layout.compression,
                            layout.compression_name()
                        ),
                    ),
                );
            }
        }
    }

    // Plane map.
    let mut per_image_missing: BTreeMap<u32, u64> = BTreeMap::new();
    for (image, c, z, t, r) in &refs {
        match r {
            None => *per_image_missing.entry(*image).or_insert(0) += 1,
            Some((file, page)) => {
                if unreadable.contains(file) {
                    continue;
                }
                let pages = parts.member_file(*file).map_or(0, |t| t.ifds.len());
                if *page >= pages {
                    let name = parts.member_name(*file);
                    rep.push(
                        "bad_tiffdata",
                        Finding::error(
                            "bad_tiffdata",
                            format!(
                                "image {image} plane c={c} z={z} t={t} refers to IFD {page} of '{name}', which has {pages} IFDs"
                            ),
                        ),
                    );
                }
            }
        }
    }
    for (image, n) in per_image_missing {
        rep.push(
            "missing_planes",
            Finding::error(
                "missing_planes",
                format!("image {image}: {n} plane(s) are not mapped to any IFD"),
            ),
        );
    }
    // Contiguous stacks (ImageJ virtual stacks, MetaMorph STK): the last plane must end inside
    // the file that holds them.
    for (&(image, file), &n) in &contiguous {
        if n == 0 || unreadable.contains(&file) {
            continue;
        }
        let plane = parts.contiguous_plane_bytes(image);
        if let (Ok(l), Some(t)) = (parts.layout(file, 0), parts.member_file(file)) {
            let start = l.offsets.first().copied().unwrap_or(0);
            let end = start.saturating_add(plane.saturating_mul(n));
            if end > t.file_len {
                let missing = (end - t.file_len).div_ceil(plane.max(1));
                let prefix = if file == 0 {
                    String::new()
                } else {
                    format!("'{}': ", parts.member_name(file))
                };
                rep.push(
                    "truncated",
                    Finding::error(
                        "truncated",
                        format!("{prefix}image {image}: the contiguous stack needs {end} bytes but the file has {} ({missing} plane(s) missing)", t.file_len),
                    )
                    .at(t.file_len),
                );
            }
        }
    }
    // UUID agreement for OME file sets.
    if matches!(parts.flavor(), Flavor::OmeTiff | Flavor::OmeCompanion) {
        for i in 0..parts.member_count() {
            if parts.member_is_metadata(i) || unreadable.contains(&i) {
                continue;
            }
            let Some(expected) = parts.member_uuid(i) else {
                continue;
            };
            let own = parts
                .member_file(i)
                .and_then(|t| t.ifds.first())
                .and_then(|p| p.text(crate::tags::IMAGE_DESCRIPTION))
                .and_then(crate::ome::parse)
                .and_then(|d| d.uuid);
            if let Some(own) = own
                && own != expected
            {
                rep.push(
                    "uuid_mismatch",
                    Finding::warning(
                        "uuid_mismatch",
                        format!(
                            "'{}' carries UUID {own} but is referenced as {expected}",
                            parts.member_name(i)
                        ),
                    ),
                );
            }
        }
    }
    if parts.flavor() == Flavor::Eer {
        eer_events(&mut parts, &mut rep);
    }
    for n in parts.notes() {
        if n.contains("acquisition stopped early") {
            rep.push(
                "short_acquisition",
                Finding::warning("short_acquisition", n.clone()),
            );
        }
    }
    rep.r
}

impl TiffDataset {
    pub(crate) fn check_parts(&mut self) -> CheckParts<'_> {
        CheckParts { ds: self }
    }
}

/// (image, c, z, t, (file, page)) of one plane; `None` when the plane is not mapped.
pub(crate) type PlaneRef = (u32, u32, u32, u32, Option<(usize, usize)>);

/// Borrowing view for the integrity checks.
pub(crate) struct CheckParts<'a> {
    ds: &'a mut TiffDataset,
}

impl CheckParts<'_> {
    pub(crate) fn path(&self) -> String {
        self.ds.path.display().to_string()
    }
    pub(crate) fn flavor(&self) -> Flavor {
        self.ds.flavor
    }
    pub(crate) fn member_count(&self) -> usize {
        self.ds.files.len()
    }
    pub(crate) fn member_name(&self, i: usize) -> String {
        self.ds.files[i].name.clone()
    }
    pub(crate) fn member_is_metadata(&self, i: usize) -> bool {
        self.ds.files[i].metadata_only
    }
    pub(crate) fn member_uuid(&self, i: usize) -> Option<String> {
        self.ds.files[i].uuid.clone()
    }
    pub(crate) fn open_member(&mut self, i: usize) -> Result<()> {
        self.ds.open_member(i)
    }
    pub(crate) fn member_file(&self, i: usize) -> Option<&TiffFile> {
        self.ds.opened(i)
    }
    pub(crate) fn layout(&self, file: usize, page: usize) -> Result<PageLayout> {
        self.ds.layout(file, page)
    }
    /// For a JPEG page: how its first written chunk would be decoded (markers read from at
    /// most 64 KiB, nothing decoded). `Ok(None)` when the page has no written chunk.
    pub(crate) fn jpeg_decision(
        &mut self,
        file: usize,
        layout: &PageLayout,
    ) -> Result<Option<crate::decode::JpegDecision>> {
        let Some((off, n)) = layout
            .offsets
            .iter()
            .zip(&layout.byte_counts)
            .find(|(o, n)| **o != 0 && **n != 0)
            .map(|(o, n)| (*o, *n))
        else {
            return Ok(None);
        };
        let Some((_, bytes)) = self.ds.files[file].opened.as_mut() else {
            return Ok(None);
        };
        let head = bytes.read_at(off, n.min(64 << 10).min(bytes.len.saturating_sub(off)))?;
        let m = openreadout_codecs::jpeg_markers(&head, layout.jpeg_tables.as_deref())
            .map_err(|e| Error::corrupt_at(FORMAT_ID, off, e.to_string()))?;
        crate::decode::jpeg_color(layout, &m).map(Some)
    }
    /// EER: the electron events of every frame page of the main file, as (page, events,
    /// pixels, recorded dose in electrons per pixel), or the page's decoding error.
    pub(crate) fn eer_event_counts(&mut self) -> Vec<(usize, Result<EerFrameEvents>)> {
        let pages: Vec<usize> = self
            .ds
            .series
            .first()
            .map(|s| {
                s.planes
                    .iter()
                    .filter_map(|p| match p {
                        Some(PlaneSrc::Page { file: 0, page, .. }) => Some(*page),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Vec::with_capacity(pages.len());
        for page in pages {
            let r = self.eer_page_events(page);
            out.push((page, r));
        }
        out
    }

    /// EER: the acquisition's `totalDose` item (electrons per pixel), when recorded.
    pub(crate) fn eer_total_dose(&self) -> Option<f64> {
        self.ds
            .main()
            .ifds
            .first()
            .and_then(|i| i.bytes(crate::eer::ACQUISITION_METADATA))
            .and_then(crate::eer::parse_items)
            .and_then(|(items, _)| items.get("totalDose").and_then(Value::as_f64))
    }

    fn eer_page_events(&mut self, page: usize) -> Result<EerFrameEvents> {
        let layout = self.ds.layout(0, page)?;
        let coding = layout
            .eer
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("page {page} is not EER-coded")))?;
        let dose = self
            .ds
            .main()
            .ifds
            .get(page)
            .and_then(|i| i.bytes(crate::eer::FRAME_METADATA))
            .and_then(crate::eer::parse_items)
            .and_then(|(items, _)| items.get("dose").and_then(Value::as_f64));
        let Some((_, src)) = self.ds.files[0].opened.as_mut() else {
            return Err(Error::corrupt(FORMAT_ID, "the file is not open"));
        };
        let (mut events, mut pixels) = (0u64, 0u64);
        let rows_per = layout.chunk_height.max(1);
        for (i, (&off, &n)) in layout.offsets.iter().zip(&layout.byte_counts).enumerate() {
            let y0 = u32::try_from(i)
                .unwrap_or(u32::MAX)
                .saturating_mul(rows_per);
            let rows = layout.height.saturating_sub(y0).min(rows_per);
            let px = u64::from(layout.width) * u64::from(rows);
            pixels += px;
            if off == 0 || n == 0 {
                continue;
            }
            let raw = src.read_at(off, n)?;
            events += crate::eer::walk(
                &raw,
                usize::try_from(px).unwrap_or(usize::MAX),
                coding,
                |_| {},
            )
            .map_err(|e| Error::corrupt_at(FORMAT_ID, off, format!("page {page}: {e}")))?;
        }
        Ok((events, pixels, dose))
    }
    /// Every mapped plane as (image, c, z, t, file, page), plus per (image, file) the number of
    /// contiguously stored planes the image reads from that file. `.nd` members are resolved
    /// first (a member that cannot be opened stays a page-0 reference, so `check` reports it).
    pub(crate) fn plane_refs(&mut self) -> (Vec<PlaneRef>, BTreeMap<(usize, usize), u64>) {
        let mut out = Vec::new();
        let mut contiguous: BTreeMap<(usize, usize), u64> = BTreeMap::new();
        let mut resolved: HashMap<(usize, u32), Option<PlaneSrc>> = HashMap::new();
        let n_series = self.ds.series.len();
        for si in 0..n_series {
            let (sz, sc, st) = {
                let i = &self.ds.series[si].info;
                (i.size_z, i.size_c, i.size_t)
            };
            for t in 0..st {
                for z in 0..sz {
                    for c in 0..sc {
                        let s = &self.ds.series[si];
                        let mut src = s.planes[s.slot(c, z, t)];
                        if let Some(PlaneSrc::Member { file, index }) = src {
                            src = *resolved.entry((file, index)).or_insert_with(|| {
                                Some(self.ds.resolve_member_plane(file, index).unwrap_or(
                                    PlaneSrc::Page {
                                        file,
                                        page: 0,
                                        sample: None,
                                    },
                                ))
                            });
                        }
                        match src {
                            Some(PlaneSrc::Page { file, page, .. }) => {
                                out.push((si as u32, c, z, t, Some((file, page))));
                            }
                            Some(PlaneSrc::Contiguous { file, index }) => {
                                let n = contiguous.entry((si, file)).or_insert(0);
                                *n = (*n).max(index + 1);
                                out.push((si as u32, c, z, t, Some((file, 0))));
                            }
                            Some(PlaneSrc::Member { .. }) | None => {
                                out.push((si as u32, c, z, t, None));
                            }
                        }
                    }
                }
            }
        }
        (out, contiguous)
    }
    pub(crate) fn contiguous_plane_bytes(&self, image: usize) -> u64 {
        let i = &self.ds.series[image].info;
        u64::from(i.size_x)
            .saturating_mul(u64::from(i.size_y))
            .saturating_mul(u64::from(i.samples_per_pixel))
            .saturating_mul(i.pixel_type.bytes_per_sample() as u64)
    }
    pub(crate) fn notes(&self) -> &[String] {
        &self.ds.notes
    }
}
