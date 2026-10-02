//! Plane, pyramid-level and region reads: where a plane is stored and how its bytes are
//! decoded.

use openreadout_core::reader::PlaneIndex;
use openreadout_core::region::{Region, ResolutionLevel};
use openreadout_core::{Error, PixelType, Plane, Result};

use crate::container::{ByteOrder, read_ifd};
use crate::dataset::{Flavor, PlaneSrc, TiffDataset, chunk_size_of};
use crate::decode::{PageLayout, SampleSelect, read_page};
use crate::{FORMAT_ID, tags};

impl TiffDataset {
    /// Plane `idx` of `image` at full resolution, read whole.
    pub(crate) fn read_stored_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        let s = self.series.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.series.len()
            ))
        })?;
        let info = &s.info;
        if idx.c >= info.size_c || idx.z >= info.size_z || idx.t >= info.size_t {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{}, z<{}, t<{})",
                idx.c, idx.z, idx.t, info.size_c, info.size_z, info.size_t
            )));
        }
        let (w, h, pt, spp) = (
            info.size_x,
            info.size_y,
            self.md_gel.as_ref().map_or(info.pixel_type, |g| g.stored),
            info.samples_per_pixel,
        );
        let src = s.planes[s.slot(idx.c, idx.z, idx.t)];
        let src = match src {
            Some(PlaneSrc::Member { file, index }) => Some(self.resolve_member_plane(file, index)?),
            other => other,
        };
        let src = src.ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "plane c={} z={} t={} of image {image} is not mapped to any IFD (incomplete OME TiffData or missing pages)",
                    idx.c, idx.z, idx.t
                ),
            )
        })?;
        let data = match src {
            PlaneSrc::Page { file, page, sample } => {
                self.open_member(file)?;
                let layout = self.layout(file, page)?;
                if layout.width != w || layout.height != h {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "page {page} of '{}' is {}×{}, the image declares {w}×{h}",
                            self.files[file].name, layout.width, layout.height
                        ),
                    ));
                }
                if layout.pixel_type()? != pt {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("page {page} has a different pixel type than image {image}"),
                    ));
                }
                let select = match sample {
                    Some(s) => SampleSelect::One(s),
                    None if spp > 1 || layout.samples_per_pixel == 1 => SampleSelect::All,
                    None => SampleSelect::One(0),
                };
                let (_, bytes) = self.files[file].opened.as_mut().expect("opened");
                read_page(bytes, &layout, select)?
            }
            PlaneSrc::Contiguous { file, index } => {
                self.open_member(file)?;
                self.read_contiguous(file, index, w, h, spp, pt)?
            }
            PlaneSrc::Member { .. } => unreachable!("resolved above"),
        };
        let (pt, data) = match &self.md_gel {
            Some(g) if data.len() == w as usize * h as usize * pt.bytes_per_sample() => {
                (PixelType::Float, g.linearize(&data))
            }
            _ => (pt, data),
        };
        let plane = Plane {
            width: w,
            height: h,
            pixel_type: pt,
            samples_per_pixel: spp,
            data,
        };
        if plane.data.len() != plane.expected_len() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "plane decodes to {} bytes, expected {}",
                    plane.data.len(),
                    plane.expected_len()
                ),
            ));
        }
        Ok(plane)
    }

    /// `(width, height)` of pyramid level `level` of `image` (level 0 = the image).
    pub(crate) fn level_dims(&self, image: u32, level: u32) -> Result<(u32, u32)> {
        let s = self.series.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.series.len()
            ))
        })?;
        if level == 0 {
            return Ok((s.info.size_x, s.info.size_y));
        }
        s.levels
            .get(level as usize - 1)
            .map(|l| (l.width, l.height))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "pyramid level {level} out of range for image {image} (0..{})",
                    s.levels.len() + 1
                ))
            })
    }

    /// Where plane `idx` of `image` is stored at level 0 (a `.nd` member is resolved).
    pub(crate) fn plane_source(&mut self, image: u32, idx: PlaneIndex) -> Result<PlaneSrc> {
        let s = self.series.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.series.len()
            ))
        })?;
        let info = &s.info;
        if idx.c >= info.size_c || idx.z >= info.size_z || idx.t >= info.size_t {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{}, z<{}, t<{})",
                idx.c, idx.z, idx.t, info.size_c, info.size_z, info.size_t
            )));
        }
        let src = s.planes.get(s.slot(idx.c, idx.z, idx.t)).copied().flatten();
        let src = match src {
            Some(PlaneSrc::Member { file, index }) => Some(self.resolve_member_plane(file, index)?),
            other => other,
        };
        src.ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "plane c={} z={} t={} of image {image} is not mapped to any IFD (incomplete OME TiffData or missing pages)",
                    idx.c, idx.z, idx.t
                ),
            )
        })
    }

    /// The layout of level `level` (≥ 1) of the plane stored on page `page` of member `file`:
    /// the page's own `SubIFDs` entry for OME-style pyramids, or the matching reduced page of
    /// the main chain for whole-slide flavours (SVS, NDPI, QPTIFF, plain reduced pages).
    pub(crate) fn level_layout(
        &mut self,
        image: u32,
        file: usize,
        page: usize,
        level: u32,
    ) -> Result<PageLayout> {
        let s = &self.series[image as usize];
        let li = level as usize - 1;
        let lv = s.levels.get(li).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "pyramid level {level} out of range for image {image} (0..{})",
                s.levels.len() + 1
            ))
        })?;
        if lv.sub_ifd.is_some() {
            // Position among the SubIFD-based levels (a plain TIFF may list reduced pages first).
            let k = s.levels[..li]
                .iter()
                .filter(|l| l.sub_ifd.is_some())
                .count();
            self.open_member(file)?;
            let t = self.opened(file).ok_or_else(|| {
                Error::corrupt(FORMAT_ID, format!("file #{file} of the set is not open"))
            })?;
            let order = t.header.byte_order;
            let off = t
                .ifds
                .get(page)
                .and_then(|i| i.uints(tags::SUB_IFDS))
                .and_then(|v| v.get(k).copied())
                .ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "page {page} of '{}' has no SubIFD for pyramid level {level} (the first plane has one)",
                            self.files[file].name
                        ),
                    )
                })?;
            if let Some(l) = self.sub_layouts.get(&(file, off)) {
                return Ok(l.clone());
            }
            let (_, src) = self.files[file]
                .opened
                .as_mut()
                .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("file #{file} is not open")))?;
            let mut probs = Vec::new();
            let ifd = read_ifd(src, off, &mut probs)?;
            let l = PageLayout::from_ifd(&ifd, order)?;
            self.sub_layouts.insert((file, off), l.clone());
            return Ok(l);
        }
        let Some(first) = lv.page else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("pyramid level {level} of image {image} has no page"),
            ));
        };
        // Level pages follow the order of the image's distinct full-resolution pages: the k-th
        // page with this level's size (from the level's first page on) belongs to the k-th one.
        let mut bases: Vec<usize> = Vec::new();
        for p in s.planes.iter().flatten() {
            if let PlaneSrc::Page { file: 0, page, .. } = p
                && !bases.contains(page)
            {
                bases.push(*page);
            }
        }
        let k = bases.iter().position(|&b| b == page).unwrap_or(0);
        if !lv.pages.is_empty() {
            let q = lv.pages.get(k).copied().ok_or_else(|| {
                Error::corrupt(
                    FORMAT_ID,
                    format!("pyramid level {level} of image {image} lists no page for this plane"),
                )
            })?;
            return self.layout(0, q);
        }
        let t = self.main();
        let candidates: Vec<usize> = (first..t.ifds.len())
            .filter(|&q| {
                let i = &t.ifds[q];
                i.uint(tags::IMAGE_WIDTH) == Some(u64::from(lv.width))
                    && i.uint(tags::IMAGE_LENGTH) == Some(u64::from(lv.height))
            })
            .collect();
        let q = candidates.get(k).copied().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!(
                    "pyramid level {level} of image {image} for this plane: the file stores the level for {} of {} planes",
                    candidates.len(),
                    bases.len()
                ),
                "Read level 0 of this plane (a region of it with --region), or a plane that has the level.",
            )
        })?;
        self.layout(0, q)
    }

    /// Region `region` of plane `idx` of `image` at pyramid `level`, decoding only the chunks
    /// it overlaps.
    pub(crate) fn read_level_region(
        &mut self,
        image: u32,
        idx: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        let (lw, lh) = self.level_dims(image, level)?;
        region.check_within(lw, lh, &format!("image {image} level {level}"))?;
        let src = self.plane_source(image, idx)?;
        let info = &self.series[image as usize].info;
        let (pt, spp) = (
            self.md_gel.as_ref().map_or(info.pixel_type, |g| g.stored),
            info.samples_per_pixel,
        );
        let data = match src {
            PlaneSrc::Page { file, page, sample } => {
                self.open_member(file)?;
                let layout = if level == 0 {
                    self.layout(file, page)?
                } else {
                    self.level_layout(image, file, page, level)?
                };
                // Philips TIFF levels are padded to whole tiles: the level is the page's top left.
                let fits = if self.flavor == Flavor::PhilipsTiff {
                    layout.width >= lw && layout.height >= lh
                } else {
                    layout.width == lw && layout.height == lh
                };
                if !fits {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "level {level} of page {page} of '{}' is {}×{}, the image declares {lw}×{lh}",
                            self.files[file].name, layout.width, layout.height
                        ),
                    ));
                }
                if layout.pixel_type()? != pt {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "page {page} level {level} has a different pixel type than image {image}"
                        ),
                    ));
                }
                let select = match sample {
                    Some(s) => SampleSelect::One(s),
                    None if spp > 1 || layout.samples_per_pixel == 1 => SampleSelect::All,
                    None => SampleSelect::One(0),
                };
                let chunks = &mut self.chunks;
                let (_, bytes) = self.files[file].opened.as_mut().ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, format!("file #{file} is not open"))
                })?;
                crate::decode::read_region(bytes, &layout, select, region, Some((chunks, file)))?
            }
            PlaneSrc::Contiguous { .. } | PlaneSrc::Member { .. } => {
                if level > 0 {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        "pyramid levels of a contiguous stack",
                        "Contiguous (ImageJ/STK) stacks have no reduced-resolution levels; read level 0.",
                    ));
                }
                let plane = self.read_stored_plane(image, idx)?;
                return openreadout_core::region::crop(plane, region, &format!("image {image}"));
            }
        };
        let (pt, data) = match &self.md_gel {
            Some(g)
                if data.len()
                    == region.width as usize * region.height as usize * pt.bytes_per_sample() =>
            {
                (PixelType::Float, g.linearize(&data))
            }
            _ => (pt, data),
        };
        let plane = Plane {
            width: region.width,
            height: region.height,
            pixel_type: pt,
            samples_per_pixel: spp,
            data,
        };
        if plane.data.len() != plane.expected_len() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "region decodes to {} bytes, expected {}",
                    plane.data.len(),
                    plane.expected_len()
                ),
            ));
        }
        Ok(plane)
    }

    /// `resolution_levels` of every image stored in tiles or with pyramid levels: sizes from
    /// the level table, tile sizes from the level's first page (or SubIFD).
    pub(crate) fn fill_resolution_levels(&mut self) {
        for si in 0..self.series.len() {
            let s = &self.series[si];
            let first = s.planes.iter().flatten().next().copied();
            let Some(PlaneSrc::Page { file: 0, page, .. }) = first else {
                continue;
            };
            let Some(t) = self.opened(0) else {
                continue;
            };
            let Some(ifd0) = t.ifds.get(page) else {
                continue;
            };
            let (w0, h0) = (s.info.size_x, s.info.size_y);
            let (tw, th) = chunk_size_of(ifd0);
            let tiled0 = ifd0.field(tags::TILE_WIDTH).is_some();
            if s.levels.is_empty() && !tiled0 {
                continue;
            }
            let mut out = vec![ResolutionLevel::new(0, w0, h0, w0, h0).with_tile(tw, th)];
            let levels = s.levels.clone();
            for (i, lv) in levels.iter().enumerate() {
                let tile = match (lv.page, lv.sub_ifd) {
                    (Some(p), _) => self
                        .opened(0)
                        .and_then(|t| t.ifds.get(p))
                        .map(chunk_size_of),
                    (None, Some(off)) => self.files[0].opened.as_mut().and_then(|(_, src)| {
                        read_ifd(src, off, &mut Vec::new())
                            .ok()
                            .map(|i| chunk_size_of(&i))
                    }),
                    (None, None) => None,
                };
                let (tw, th) = tile.unwrap_or((0, 0));
                out.push(
                    ResolutionLevel::new(i as u32 + 1, lv.width, lv.height, w0, h0)
                        .with_tile(tw, th),
                );
            }
            self.series[si].info.resolution_levels = out;
        }
    }

    /// The `(file, index)` plane of a `.nd` member, once the member is open: an STK stack is
    /// read contiguously, any other TIFF page by page.
    pub(crate) fn resolve_member_plane(&mut self, file: usize, index: u32) -> Result<PlaneSrc> {
        self.open_member(file)?;
        let t = self.opened(file).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("file #{file} of the set is not open"))
        })?;
        let Some(page0) = t.ifds.first() else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("'{}' has no readable page", self.files[file].name),
            ));
        };
        let stk_planes = if crate::metamorph::is_stk(page0) {
            page0
                .field(crate::metamorph::STK_TAG_PLANES)
                .map(|f| f.count)
        } else {
            None
        };
        if let Some(n) = stk_planes {
            if u64::from(index) >= n {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "'{}' holds {n} planes; plane {index} was asked for",
                        self.files[file].name
                    ),
                ));
            }
            return Ok(PlaneSrc::Contiguous {
                file,
                index: u64::from(index),
            });
        }
        if index as usize >= t.ifds.len() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "'{}' has {} page(s); plane {index} was asked for",
                    self.files[file].name,
                    t.ifds.len()
                ),
            ));
        }
        Ok(PlaneSrc::Page {
            file,
            page: index as usize,
            sample: None,
        })
    }

    /// Plane `index` of a stack stored back to back from page 0's first strip of `file`.
    pub(crate) fn read_contiguous(
        &mut self,
        file: usize,
        index: u64,
        w: u32,
        h: u32,
        spp: u32,
        pt: PixelType,
    ) -> Result<Vec<u8>> {
        let layout = self.layout(file, 0)?;
        let lpt = layout.pixel_type()?;
        if layout.width != w || layout.height != h || lpt != pt {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "'{}' is {}×{} {}, the image declares {w}×{h} {}",
                    self.files[file].name,
                    layout.width,
                    layout.height,
                    lpt.ome_name(),
                    pt.ome_name()
                ),
            ));
        }
        let bps = pt.bytes_per_sample() as u64;
        let plane = u64::from(w)
            .saturating_mul(u64::from(h))
            .saturating_mul(u64::from(spp))
            .saturating_mul(bps);
        if plane > crate::decode::MAX_PLANE_BYTES {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a {w}×{h} plane larger than 4 GiB"),
                "Planes this large are not read into memory.",
            ));
        }
        let off = index
            .checked_mul(plane)
            .and_then(|o| o.checked_add(layout.offsets.first().copied().unwrap_or(0)))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "contiguous plane offset overflows"))?;
        let name = self.files[file].name.clone();
        let (_, bytes) = self.files[file]
            .opened
            .as_mut()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("'{name}' is not open")))?;
        let mut d = bytes.read_at(off, plane).map_err(|_| {
            Error::corrupt_at(
                FORMAT_ID,
                off,
                format!(
                    "plane {index} of the contiguous stack in '{name}' runs past the end of the file (truncated?)"
                ),
            )
        })?;
        if layout.byte_order == ByteOrder::Big && bps > 1 {
            for c in d.chunks_exact_mut(bps as usize) {
                c.reverse();
            }
        }
        Ok(d)
    }
}
