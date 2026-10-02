//! `Dataset` implementation: sub-format detection, image (series) assembly, plane reads,
//! listing and integrity checks. See `docs/formats/tiff.md`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, ImageInfo, InstrumentInfo, LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::Region;
use openreadout_core::source::{Fs, Input};
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Map, Value, json};

use crate::container::{ByteOrder, FieldValue, Ifd, TiffFile, read_ifd};
use crate::decode::{ChunkCache, PageLayout};
use crate::files::{FileSetMember, file_name_of};
use crate::flavors::{parse_aperio, parse_micromanager, parse_qpi};
use crate::imagej::{parse_binary, parse_description};
use crate::lsm::lsm_record;
use crate::metamorph::{NdFile, StkInfo, parse_metaseries, parse_stk};
use crate::nis::parse_nis;
use crate::ome::read_companion;
use crate::{FORMAT_ID, TiffReader, tags};

mod growing;
mod listing;
mod read;

/// Which convention the file follows. Detected from tags, in this priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    OmeTiff,
    OmeCompanion,
    Lsm,
    Svs,
    Ndpi,
    Qptiff,
    ImageJ,
    MicroManager,
    /// Leica SCN whole-slide files.
    LeicaScn,
    /// Roche Ventana BIF whole-slide files.
    VentanaBif,
    /// Philips TIFF whole-slide exports.
    PhilipsTiff,
    MetamorphStk,
    MetaSeries,
    MetamorphNd,
    NisElements,
    /// Thermo Fisher EER electron-event movies (Falcon cameras).
    Eer,
    Plain,
}

impl Flavor {
    pub fn id(self) -> &'static str {
        match self {
            Flavor::OmeTiff => "ome-tiff",
            Flavor::OmeCompanion => "ome-companion",
            Flavor::Lsm => "zeiss-lsm",
            Flavor::Svs => "aperio-svs",
            Flavor::Ndpi => "hamamatsu-ndpi",
            Flavor::Qptiff => "perkinelmer-qptiff",
            Flavor::ImageJ => "imagej",
            Flavor::MicroManager => "micro-manager",
            Flavor::LeicaScn => "leica-scn",
            Flavor::VentanaBif => "ventana-bif",
            Flavor::PhilipsTiff => "philips-tiff",
            Flavor::MetamorphStk => "metamorph-stk",
            Flavor::MetaSeries => "metaseries",
            Flavor::MetamorphNd => "metamorph-nd",
            Flavor::NisElements => "nis-elements",
            Flavor::Eer => "thermo-eer",
            Flavor::Plain => "plain",
        }
    }
}

/// Where the samples of one (c, z, t) plane live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneSrc {
    /// A whole page (or one sample of it) in member `file`.
    Page {
        file: usize,
        page: usize,
        sample: Option<u16>,
    },
    /// ImageJ "virtual" stacks and MetaMorph STK files: planes stored back to back from the
    /// start of page 0's data in member `file`.
    Contiguous { file: usize, index: u64 },
    /// Plane `index` of member `file` of a MetaMorph `.nd` series: resolved when the member is
    /// opened (an STK stack → `Contiguous`, a multi-page TIFF → `Page`).
    Member { file: usize, index: u32 },
}

#[derive(Debug, Clone)]
pub(crate) struct Level {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Main-chain page index, or a SubIFD offset.
    pub(crate) page: Option<usize>,
    pub(crate) sub_ifd: Option<u64>,
    /// The level's page for each full-resolution page of the image, in the order the image's
    /// planes first use them (Leica SCN lists them); empty: found by size from `page` on.
    pub(crate) pages: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Attachment {
    pub(crate) role: String,
    pub(crate) page: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Series {
    pub(crate) info: ImageInfo,
    /// Indexed by `(t * size_z + z) * size_c + c`.
    pub(crate) planes: Vec<Option<PlaneSrc>>,
    /// Reduced-resolution levels below the full-resolution image.
    pub(crate) levels: Vec<Level>,
}

impl Series {
    pub(crate) fn slot(&self, c: u32, z: u32, t: u32) -> usize {
        ((t as usize * self.info.size_z as usize) + z as usize) * self.info.size_c as usize
            + c as usize
    }
}

/// An opened TIFF-family file (and, for multi-file OME-TIFF, its siblings).
#[derive(Debug)]
pub struct TiffDataset {
    pub(crate) path: PathBuf,
    /// Where the file and its file-set members are read from.
    pub(crate) fs: Fs,
    pub(crate) file_len: u64,
    pub(crate) files: Vec<FileSetMember>,
    pub(crate) flavor: Flavor,
    pub(crate) series: Vec<Series>,
    pub(crate) attachments: Vec<Attachment>,
    /// Associated images stored inside the metadata (Philips TIFF: base64 JPEGs in the XML):
    /// name and bytes.
    pub(crate) embedded: Vec<(String, Vec<u8>)>,
    pub(crate) notes: Vec<String>,
    pub(crate) vendor: Value,
    /// OME-XML text and where it came from (file member, byte offset of the value).
    pub(crate) ome_xml: Option<(usize, Option<u64>, String)>,
    /// Schema file name of the OME-XML, recorded when the document is parsed at open (so
    /// `info` does not parse it a second time); valid once `ome_parsed` is set.
    pub(crate) ome_schema: Option<String>,
    pub(crate) ome_parsed: bool,
    /// LSM: corrected strip byte counts per page (the tag stores uncompressed sizes).
    pub(crate) byte_count_fix: HashMap<usize, Vec<u64>>,
    /// MetaMorph STK metadata of the opened file.
    pub(crate) stk: Option<StkInfo>,
    /// MetaMorph `.nd` series description, when a `.nd` file was opened.
    pub(crate) nd: Option<NdFile>,
    pub(crate) provenance: ProvenanceMap,
    /// Layouts of pyramid-level IFDs reached through `SubIFDs`, by (member, IFD offset).
    pub(crate) sub_layouts: HashMap<(usize, u64), PageLayout>,
    /// Decoded chunks kept between region reads.
    pub(crate) chunks: ChunkCache,
    /// Molecular Dynamics GEL encoding: planes are returned as counts (float32).
    pub(crate) md_gel: Option<crate::mdgel::MdGel>,
}

const NUMERIC_KEY_LIMIT: usize = 64;

/// Decoded chunks kept between region reads (dask chunks and export blocks overlap strips).
const REGION_CHUNK_CACHE_BYTES: usize = 64 << 20;

/// Stored tile size of a page: the tile, or (width, rows per strip) for strips.
pub(crate) fn chunk_size_of(ifd: &Ifd) -> (u32, u32) {
    let w = ifd.uint(tags::IMAGE_WIDTH).unwrap_or(0);
    let h = ifd.uint(tags::IMAGE_LENGTH).unwrap_or(0);
    let (tw, th) = if ifd.field(tags::TILE_WIDTH).is_some() {
        (
            ifd.uint(tags::TILE_WIDTH).unwrap_or(0),
            ifd.uint(tags::TILE_LENGTH).unwrap_or(0),
        )
    } else {
        (w, ifd.uint(tags::ROWS_PER_STRIP).unwrap_or(h).min(h))
    };
    (
        u32::try_from(tw).unwrap_or(0),
        u32::try_from(th).unwrap_or(0),
    )
}

pub(crate) fn text_value(ifd: &Ifd, tag: u16) -> Option<String> {
    ifd.text(tag)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Physical pixel size from XResolution/YResolution (pixels per unit). Resolution unit 1
/// ("no absolute unit") gives nothing; common screen/print defaults in inches are ignored.
pub(crate) fn resolution_um(ifd: &Ifd) -> (Option<f64>, Option<f64>) {
    let factor = match ifd.uint(tags::RESOLUTION_UNIT).unwrap_or(2) {
        2 => 25_400.0,
        3 => 10_000.0,
        _ => return (None, None),
    };
    let inch = factor > 20_000.0;
    let conv = |tag: u16| {
        ifd.float(tag)
            .filter(|r| r.is_finite() && *r > 0.0)
            .and_then(|r| {
                if inch && [1.0, 72.0, 96.0, 150.0, 300.0, 600.0].contains(&r) {
                    None
                } else {
                    Some(factor / r)
                }
            })
    };
    (conv(tags::X_RESOLUTION), conv(tags::Y_RESOLUTION))
}

fn field_json(v: &FieldValue) -> Value {
    fn cut<T: Clone + Into<Value>>(v: &[T]) -> Value {
        if v.len() == 1 {
            v[0].clone().into()
        } else if v.len() <= NUMERIC_KEY_LIMIT {
            Value::Array(v.iter().cloned().map(Into::into).collect())
        } else {
            json!({"count": v.len(), "first": v[..8].iter().cloned().map(Into::into).collect::<Vec<Value>>()})
        }
    }
    match v {
        FieldValue::Unsigned(v) => cut(v),
        FieldValue::Signed(v) => cut(v),
        FieldValue::Float(v) => cut(v),
        FieldValue::Ascii(s) => {
            if s.len() > 65_536 {
                json!({"length": s.len(), "start": s.chars().take(1024).collect::<String>()})
            } else {
                Value::String(s.clone())
            }
        }
        FieldValue::Bytes(b) => json!({"bytes": b.len()}),
    }
}

fn ifd_json(ifd: &Ifd) -> Value {
    let mut m = Map::new();
    for f in &ifd.fields {
        let key = tags::name(f.tag).map_or_else(|| f.tag.to_string(), str::to_string);
        m.insert(key, field_json(&f.value));
    }
    Value::Object(m)
}

impl TiffDataset {
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let is_xml = crate::is_companion_path(path) || !crate::file_starts_like_tiff(fs, path)?;
        let mut ds = Self::empty(path, fs)?;
        if crate::is_nd_path(fs, path) {
            ds.open_nd()?;
        } else if is_xml {
            ds.open_companion()?;
        } else {
            ds.open_tiff()?;
        }
        Ok(ds)
    }

    fn empty(path: &Path, fs: &Fs) -> Result<Self> {
        Ok(TiffDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file_len: fs.metadata(path).map_err(|e| Error::io(path, e))?.len(),
            files: Vec::new(),
            flavor: Flavor::Plain,
            series: Vec::new(),
            attachments: Vec::new(),
            embedded: Vec::new(),
            notes: Vec::new(),
            vendor: Value::Null,
            ome_xml: None,
            ome_schema: None,
            ome_parsed: false,
            byte_count_fix: HashMap::new(),
            stk: None,
            nd: None,
            provenance: ProvenanceMap::new(),
            sub_layouts: HashMap::new(),
            chunks: ChunkCache::new(REGION_CHUNK_CACHE_BYTES),
            md_gel: None,
        })
    }

    /// A companion OME-XML file (`.companion.ome`) of a multi-file OME-TIFF set.
    fn open_companion(&mut self) -> Result<()> {
        let xml = read_companion(&self.fs, &self.path)?;
        let doc = crate::ome::parse(&xml).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, "companion file is not an OME-XML document")
        })?;
        self.files.push(FileSetMember {
            path: self.path.clone(),
            name: file_name_of(&self.path),
            uuid: doc.uuid.clone(),
            metadata_only: true,
            opened: None,
            error: None,
        });
        self.flavor = Flavor::OmeCompanion;
        self.ome_xml = Some((0, None, xml));
        self.build_ome(&doc, None)?;
        self.finish_vendor(None);
        Ok(())
    }

    /// A TIFF file: detect its convention from page 0, build its images, then add the notes
    /// that apply to every convention.
    fn open_tiff(&mut self) -> Result<()> {
        let opened = TiffFile::open_in(&self.fs, &self.path)?;
        self.files.push(FileSetMember::tiff(
            self.path.clone(),
            file_name_of(&self.path),
            opened,
        ));
        let (page0, page_count) = {
            let t = self.main();
            if t.ifds.is_empty() {
                let why = t
                    .problems
                    .first()
                    .map_or_else(|| "no IFDs".to_string(), |p| p.detail.clone());
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    t.header.first_ifd_offset,
                    format!("the first IFD cannot be read: {why}"),
                ));
            }
            (t.ifds[0].clone(), t.ifds.len())
        };
        self.build_detected(&page0)?;
        self.apply_page0_conventions(&page0);
        if self.series.is_empty() {
            return Err(Error::unsupported(
                FORMAT_ID,
                "a TIFF with no decodable image",
                "Every page uses a sample layout this reader does not decode (see known_gaps); `info --view structure` and `check` still work on the structure.",
            ));
        }
        self.note_tiff_caveats(page_count);
        self.finish_vendor(Some(&page0));
        self.fill_resolution_levels();
        Ok(())
    }

    /// Detects the convention, in priority order, and builds its images. A convention whose
    /// metadata describes no readable image (OME, SCN, ImageJ) falls through to the next one;
    /// plain TIFF (or Micro-Manager) is the last.
    fn build_detected(&mut self, page0: &Ifd) -> Result<()> {
        let desc = page0
            .text(tags::IMAGE_DESCRIPTION)
            .unwrap_or_default()
            .to_string();
        let software = page0.text(tags::SOFTWARE).unwrap_or_default();
        if (desc.contains("<OME") || desc.contains(":OME")) && self.try_ome(page0, &desc)? {
            return Ok(());
        }
        if let Some(rec) = lsm_record(&mut self.files[0], page0) {
            self.flavor = Flavor::Lsm;
            return self.build_lsm(&rec);
        }
        if crate::metamorph::is_stk(page0) {
            let stk = {
                let (_, src) = self.files[0].opened.as_mut().expect("opened");
                parse_stk(page0, src)
            };
            if let Some(stk) = stk {
                self.flavor = Flavor::MetamorphStk;
                self.build_stk(page0, &stk)?;
                self.stk = Some(stk);
                return Ok(());
            }
        }
        if page0
            .uint(tags::COMPRESSION)
            .is_some_and(|c| crate::eer::is_eer_compression(c as u16))
        {
            self.flavor = Flavor::Eer;
            return self.build_eer(page0);
        }
        if let Some(nis) = parse_nis(page0) {
            self.flavor = Flavor::NisElements;
            return self.build_nis(page0, &nis);
        }
        if let Some(ap) = parse_aperio(&desc) {
            self.flavor = Flavor::Svs;
            return self.build_svs(&ap);
        }
        if let Some(scn) = crate::scn::parse_scn(&desc) {
            self.flavor = Flavor::LeicaScn;
            self.build_scn(&scn)?;
            if !self.series.is_empty() {
                return Ok(());
            }
        }
        if software.starts_with("Philips")
            && let Some(ph) = crate::philips::parse_philips(&desc)
        {
            self.flavor = Flavor::PhilipsTiff;
            return self.build_philips(&ph);
        }
        if let Some(iscan) = self.bif_iscan() {
            self.flavor = Flavor::VentanaBif;
            return self.build_bif(&iscan);
        }
        if page0.field(tags::NDPI_FORMAT_FLAG).is_some() {
            self.flavor = Flavor::Ndpi;
            return self.build_ndpi();
        }
        if software.starts_with("PerkinElmer-QPI")
            || desc.contains("PerkinElmer-QPI-ImageDescription")
        {
            self.flavor = Flavor::Qptiff;
            return self.build_qptiff();
        }
        if software == "MetaSeries"
            && let Some(ms) = parse_metaseries(&desc)
        {
            self.flavor = Flavor::MetaSeries;
            self.build_plain()?;
            self.apply_metaseries(&ms);
            return Ok(());
        }
        if let Some(ij) = parse_description(&desc) {
            self.flavor = Flavor::ImageJ;
            self.build_imagej(&ij, page0)?;
            if !self.series.is_empty() {
                return Ok(());
            }
        }
        self.flavor = if page0.field(tags::MICROMANAGER_META).is_some() {
            Flavor::MicroManager
        } else {
            Flavor::Plain
        };
        self.build_plain()
    }

    /// Conventions layered on whatever was detected: Micro-Manager plane metadata, EER
    /// metadata on a page that is not EER-coded, Molecular Dynamics GEL scaling.
    fn apply_page0_conventions(&mut self, page0: &Ifd) {
        if let Some(mm) = page0
            .text(tags::MICROMANAGER_META)
            .and_then(parse_micromanager)
        {
            self.apply_micromanager(&mm);
        }
        if self.flavor != Flavor::Eer && crate::eer::has_metadata(page0) {
            self.apply_eer_metadata(page0);
        }
        if self.flavor == Flavor::Plain && page0.field(crate::mdgel::MD_FILE_TAG).is_some() {
            self.apply_md_gel(page0);
        }
    }

    /// Notes on truncation, undecoded compressions and guessed page order.
    fn note_tiff_caveats(&mut self, page_count: usize) {
        if self.main().problems.iter().any(|p| p.code == "truncated") {
            self.notes
                .push("file appears truncated; run `check`".into());
        }
        for i in 0..self.series.len() {
            if let Some(PlaneSrc::Page { file: 0, page, .. }) =
                self.series[i].planes.iter().flatten().next().copied()
                && let Ok(l) = self.layout(0, page)
                && !crate::decode::is_decoded(l.compression)
            {
                let msg = format!(
                    "image {i}: pixel data uses {} compression, which is not decoded (metadata and check still work)",
                    l.compression_name()
                );
                self.notes.push(msg);
            }
        }
        if page_count > 1 && self.flavor == Flavor::Plain {
            let grouped: u32 = self.series.iter().map(|s| s.info.size_z).sum();
            if grouped > self.series.len() as u32 {
                self.notes.push("no metadata describes the page order: pages with identical geometry are exposed as Z planes of one image".into());
            }
        }
    }

    pub(crate) fn opened(&self, file: usize) -> Option<&TiffFile> {
        self.files.get(file)?.opened.as_ref().map(|(t, _)| t)
    }

    pub(crate) fn main(&self) -> &TiffFile {
        self.opened(0).expect("main file is a TIFF")
    }

    pub(crate) fn layout(&self, file: usize, page: usize) -> Result<PageLayout> {
        let t = self.opened(file).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("file #{file} of the set is not open"))
        })?;
        let ifd = t.ifds.get(page).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "page {page} does not exist in '{}' ({} pages)",
                    self.files[file].name,
                    t.ifds.len()
                ),
            )
        })?;
        let mut l = PageLayout::from_ifd(ifd, t.header.byte_order)?;
        if file == 0
            && let Some(fix) = self.byte_count_fix.get(&page)
        {
            l.byte_counts.clone_from(fix);
        }
        if self.flavor == Flavor::PhilipsTiff {
            // Unscanned tiles are white in the scanner's own downsampled levels.
            l.sparse_fill = 255;
        }
        Ok(l)
    }

    /// Records where normalized fields come from.
    pub(crate) fn set_provenance(&mut self, fields: &[(&str, Source)]) {
        for (k, s) in fields {
            self.provenance.insert((*k).to_string(), *s);
        }
    }

    /// Pyramid levels stored as the `SubIFDs` of page `page` of member `file` (none when the
    /// member is not open).
    pub(crate) fn sub_ifd_levels(&mut self, file: usize, page: usize) -> Vec<Level> {
        let Some(subs) = self
            .opened(file)
            .and_then(|t| t.ifds.get(page))
            .and_then(|i| i.uints(tags::SUB_IFDS))
        else {
            return Vec::new();
        };
        let Some((_, src)) = self.files[file].opened.as_mut() else {
            return Vec::new();
        };
        let mut probs = Vec::new();
        subs.into_iter()
            .filter_map(|off| {
                let sub = read_ifd(src, off, &mut probs).ok()?;
                Some(Level {
                    width: sub.uint(tags::IMAGE_WIDTH).unwrap_or(0) as u32,
                    height: sub.uint(tags::IMAGE_LENGTH).unwrap_or(0) as u32,
                    page: None,
                    sub_ifd: Some(off),
                    pages: Vec::new(),
                })
            })
            .collect()
    }

    // ---------------------------------------------------------------- plain TIFF

    /// Plain TIFF: pages with the same geometry and sample layout are one image, as Z planes.
    /// Reduced-resolution pages after an image's last page, and its `SubIFDs`, are its
    /// pyramid levels; reduced pages no image takes are attachments.
    fn build_plain(&mut self) -> Result<()> {
        let ifds = self.main().ifds.clone();
        let (groups, reduced) = self.group_plain_pages(&ifds)?;
        for pages in &groups {
            self.push_plain_image(&ifds, pages, &reduced)?;
        }
        let used: Vec<usize> = self
            .series
            .iter()
            .flat_map(|s| s.levels.iter().filter_map(|l| l.page))
            .collect();
        for r in reduced {
            if !used.contains(&r) {
                self.attachments.push(Attachment {
                    role: "reduced-resolution".into(),
                    page: r,
                });
            }
        }
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::Inferred),
            ("images[].size_c", Source::Spec),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::Spec),
            ("images[].acquired_at", Source::Spec),
            ("images[].instrument", Source::Spec),
        ]);
        Ok(())
    }

    /// The full-resolution pages grouped by geometry and sample layout (in page order), and
    /// the reduced-resolution pages. Pages with a layout the reader does not decode are
    /// skipped with a note; when no page is left that is an error.
    fn group_plain_pages(&mut self, ifds: &[Ifd]) -> Result<(Vec<Vec<usize>>, Vec<usize>)> {
        let order = self.main().header.byte_order;
        let mut groups: Vec<(PageKey, Vec<usize>)> = Vec::new();
        let mut unsupported = Vec::new();
        let mut reduced: Vec<usize> = Vec::new();
        for (i, ifd) in ifds.iter().enumerate() {
            if ifd.uint(tags::NEW_SUBFILE_TYPE).unwrap_or(0) & 1 == 1 {
                reduced.push(i);
                continue;
            }
            let l = match PageLayout::from_ifd(ifd, order) {
                Ok(l) => l,
                Err(e) => {
                    unsupported.push(format!("page {i}: {e}"));
                    continue;
                }
            };
            if let Err(e) = l.pixel_type() {
                unsupported.push(format!("page {i}: {e}"));
                continue;
            }
            let key = PageKey::of(&l);
            if let Some(g) = groups.iter_mut().find(|g| g.0 == key) {
                g.1.push(i);
            } else {
                groups.push((key, vec![i]));
            }
        }
        if !unsupported.is_empty() {
            self.notes.push(format!(
                "{} page(s) skipped: {}",
                unsupported.len(),
                unsupported
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            if groups.is_empty() {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    unsupported[0].clone(),
                    "No page of this TIFF uses a sample layout the reader decodes.",
                ));
            }
        }
        Ok((
            groups.into_iter().map(|(_, pages)| pages).collect(),
            reduced,
        ))
    }

    /// One plain-TIFF image from a group of same-layout pages.
    fn push_plain_image(&mut self, ifds: &[Ifd], pages: &[usize], reduced: &[usize]) -> Result<()> {
        let first = &ifds[pages[0]];
        let l = PageLayout::from_ifd(first, self.main().header.byte_order)?;
        let index = self.series.len() as u32;
        let mut info = ImageInfo::new(index, l.width, l.height, l.pixel_type()?);
        let per_sample = l.planar == 2 && l.samples_per_pixel > 1;
        info.size_z = pages.len() as u32;
        info.size_c = if per_sample {
            u32::from(l.samples_per_pixel)
        } else {
            1
        };
        info.samples_per_pixel = if per_sample {
            1
        } else {
            u32::from(l.samples_per_pixel)
        };
        let (x, y) = resolution_um(first);
        info.physical_size = PhysicalSize::micrometres(x, y, None);
        info.name = text_value(first, tags::IMAGE_DESCRIPTION)
            .filter(|d| d.len() <= 256 && !d.starts_with('{') && !d.contains('\n'));
        info.channels = (0..info.size_c)
            .map(|c| ChannelInfo {
                index: c,
                ..ChannelInfo::default()
            })
            .collect();
        info.instrument = instrument_from_tags(first);
        info.acquired_at = text_value(first, tags::DATE_TIME).and_then(|d| tiff_datetime(&d));
        info.extra.insert(
            "pages".into(),
            json!(if pages.len() > 64 {
                json!({"first": pages[0], "count": pages.len()})
            } else {
                json!(pages)
            }),
        );
        info.extra
            .insert("compression".into(), json!(l.compression_name()));
        let mut planes = Vec::new();
        for &p in pages {
            for c in 0..info.size_c {
                planes.push(Some(PlaneSrc::Page {
                    file: 0,
                    page: p,
                    sample: per_sample.then_some(c as u16),
                }));
            }
        }
        // Reduced-resolution pages right after this group's last page become its levels.
        let mut levels = Vec::new();
        for &r in reduced {
            if r > *pages.last().unwrap_or(&0)
                && ifds[r]
                    .uint(tags::IMAGE_WIDTH)
                    .is_some_and(|w| w < u64::from(l.width))
            {
                levels.push(Level {
                    width: ifds[r].uint(tags::IMAGE_WIDTH).unwrap_or(0) as u32,
                    height: ifds[r].uint(tags::IMAGE_LENGTH).unwrap_or(0) as u32,
                    page: Some(r),
                    sub_ifd: None,
                    pages: Vec::new(),
                });
            }
        }
        levels.extend(self.sub_ifd_levels(0, pages[0]));
        info.pyramid_levels = 1 + levels.len() as u32;
        self.series.push(Series {
            info: info.finish(),
            planes,
            levels,
        });
        Ok(())
    }

    // ---------------------------------------------------------------- whole-slide images

    /// Shared by SVS/NDPI/QPTIFF: one baseline image with focal planes and levels.
    pub(crate) fn push_slide(
        &mut self,
        name: Option<String>,
        z_pages: &[usize],
        channel_pages: &[usize],
        levels: Vec<Level>,
    ) -> Result<ImageInfo> {
        let t = self.main();
        let order = t.header.byte_order;
        let base = z_pages
            .first()
            .or(channel_pages.first())
            .copied()
            .unwrap_or(0);
        let l = PageLayout::from_ifd(&t.ifds[base], order)?;
        let pt = l.pixel_type()?;
        let mut info = ImageInfo::new(self.series.len() as u32, l.width, l.height, pt);
        info.name = name;
        let per_sample = l.planar == 2 && l.samples_per_pixel > 1;
        info.size_z = z_pages.len().max(1) as u32;
        info.size_c = if channel_pages.len() > 1 {
            channel_pages.len() as u32
        } else if per_sample {
            u32::from(l.samples_per_pixel)
        } else {
            1
        };
        info.samples_per_pixel = if per_sample || channel_pages.len() > 1 {
            1
        } else {
            u32::from(l.samples_per_pixel)
        };
        let (x, y) = resolution_um(&t.ifds[base]);
        info.physical_size = PhysicalSize::micrometres(x, y, None);
        info.channels = (0..info.size_c)
            .map(|c| ChannelInfo {
                index: c,
                ..ChannelInfo::default()
            })
            .collect();
        info.extra
            .insert("compression".into(), json!(l.compression_name()));
        if l.tiled {
            info.extra
                .insert("tile_size".into(), json!([l.chunk_width, l.chunk_height]));
        }
        let mut planes = Vec::new();
        if channel_pages.len() > 1 {
            for &p in channel_pages {
                planes.push(Some(PlaneSrc::Page {
                    file: 0,
                    page: p,
                    sample: None,
                }));
            }
        } else {
            for &p in if z_pages.is_empty() {
                channel_pages
            } else {
                z_pages
            } {
                for c in 0..info.size_c {
                    planes.push(Some(PlaneSrc::Page {
                        file: 0,
                        page: p,
                        sample: per_sample.then_some(c as u16),
                    }));
                }
            }
        }
        info.pyramid_levels = 1 + levels.len() as u32;
        if !levels.is_empty() {
            info.extra.insert(
                "levels".into(),
                json!(
                    levels
                        .iter()
                        .map(|lv| [lv.width, lv.height])
                        .collect::<Vec<_>>()
                ),
            );
        }
        let info = info.finish();
        self.series.push(Series {
            info: info.clone(),
            planes,
            levels,
        });
        Ok(info)
    }

    pub(crate) fn page_dims(&self, p: usize) -> (u32, u32) {
        let i = &self.main().ifds[p];
        (
            i.uint(tags::IMAGE_WIDTH).unwrap_or(0) as u32,
            i.uint(tags::IMAGE_LENGTH).unwrap_or(0) as u32,
        )
    }

    pub(crate) fn replace_last(&mut self, info: ImageInfo) {
        if let Some(s) = self.series.last_mut() {
            s.info = info;
        }
    }

    fn finish_vendor(&mut self, page0: Option<&Ifd>) {
        let mut v = Map::new();
        if let Some((t, _)) = self.files.first().and_then(|f| f.opened.as_ref()) {
            v.insert(
                "tiff".into(),
                json!({
                    "byte_order": if t.header.byte_order == ByteOrder::Little { "little" } else { "big" },
                    "big_tiff": t.header.big_tiff,
                    "page_count": t.ifds.len(),
                    "first_page": page0.map(ifd_json),
                }),
            );
        }
        // The OME-XML part ("ome", right after "tiff") is converted in `vendor_metadata`, on
        // demand: `info` never needs it and the conversion is the costliest step of opening
        // an OME-TIFF.
        if let Some(p) = page0 {
            let desc = p.text(tags::IMAGE_DESCRIPTION).unwrap_or_default();
            match self.flavor {
                Flavor::ImageJ => {
                    if let Some(ij) = parse_description(desc) {
                        v.insert("imagej".into(), Value::Object(ij.keys));
                    }
                    if let (Some(c), Some(d)) = (
                        p.uints(tags::IMAGEJ_META_COUNTS),
                        p.bytes(tags::IMAGEJ_META),
                    ) {
                        v.insert("imagej_binary".into(), Value::Object(parse_binary(&c, d)));
                    }
                }
                Flavor::Svs => {
                    if let Some(ap) = parse_aperio(desc) {
                        v.insert(
                            "aperio".into(),
                            json!({"header": ap.header, "keys": ap.keys}),
                        );
                    }
                }
                Flavor::Ndpi => {
                    let mut m = Map::new();
                    for f in p.fields.iter().filter(|f| f.tag >= 65_000) {
                        m.insert(f.tag.to_string(), field_json(&f.value));
                    }
                    v.insert("ndpi_tags".into(), Value::Object(m));
                }
                Flavor::MetamorphStk => {
                    if let Some(stk) = &self.stk {
                        v.insert("metamorph_stk".into(), stk.to_json());
                    }
                }
                Flavor::MetaSeries => {
                    if let Some(ms) = parse_metaseries(desc) {
                        v.insert("metaseries".into(), Value::Object(ms.props));
                    }
                }
                Flavor::NisElements => {
                    if let Some(n) = parse_nis(p) {
                        v.insert(
                            "nis_elements".into(),
                            json!({
                                "time_ms": n.time_ms,
                                "pixel_size_um": n.pixel_size_um,
                                "stage_um": n.stage_um,
                                "tag_65327": n.unknown[0],
                                "tag_65328": n.unknown[1],
                            }),
                        );
                    }
                }
                Flavor::LeicaScn => {
                    if let Some(doc) = crate::scn::parse_scn(desc) {
                        v.insert(
                            "scn".into(),
                            json!({
                                "collection": doc.collection_name,
                                "uuid": doc.uuid,
                                "collection_size_nm": doc.collection_size_nm.map(|c| [c.0, c.1]),
                                "barcode": doc.barcode,
                            }),
                        );
                    }
                }
                Flavor::Qptiff => {
                    let pages: Vec<Value> = self
                        .main()
                        .ifds
                        .iter()
                        .take(256)
                        .filter_map(|i| i.text(tags::IMAGE_DESCRIPTION).and_then(parse_qpi))
                        .map(|q| Value::Object(q.fields))
                        .collect();
                    v.insert("qpi_pages".into(), Value::Array(pages));
                }
                _ => {}
            }
            if let Some(mm) = p.text(tags::MICROMANAGER_META).and_then(parse_micromanager) {
                v.insert("micromanager".into(), Value::Object(mm.json));
            }
        }
        self.vendor = Value::Object(v);
    }

    /// Open every file-set member (for `check` and for reads).
    pub(crate) fn open_member(&mut self, file: usize) -> Result<()> {
        self.files
            .get_mut(file)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("file #{file} is not in the set")))?
            .ensure_open(&self.fs)
            .map(|_| ())
    }
}

/// Largest plane table we build (header-declared sizes are not trusted blindly).
const MAX_PLANES: u64 = 1 << 23;

pub(crate) fn check_plane_count(n: u64) -> Result<()> {
    if n > MAX_PLANES {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("an image with {n} planes"),
            "Images with more than 8 388 608 (c, z, t) planes are not supported; the declared size may also be corrupt (run `check`).",
        ));
    }
    Ok(())
}

/// Grouping key for pages of a plain TIFF.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageKey {
    width: u32,
    height: u32,
    spp: u16,
    bits: u16,
    format: u16,
    planar: u16,
    photometric: u16,
}

impl PageKey {
    fn of(l: &PageLayout) -> Self {
        PageKey {
            width: l.width,
            height: l.height,
            spp: l.samples_per_pixel,
            bits: l.bits_per_sample,
            format: l.sample_format,
            planar: l.planar,
            photometric: l.photometric,
        }
    }
}

pub(crate) fn instrument_from_tags(ifd: &Ifd) -> Option<InstrumentInfo> {
    let make = text_value(ifd, tags::MAKE);
    let model = text_value(ifd, tags::MODEL);
    let software = text_value(ifd, tags::SOFTWARE);
    (make.is_some() || model.is_some() || software.is_some()).then(|| InstrumentInfo {
        manufacturer: make,
        model,
        software,
        ..InstrumentInfo::default()
    })
}

/// TIFF DateTime `YYYY:MM:DD HH:MM:SS` → ISO-8601 (local time, no zone).
pub(crate) fn tiff_datetime(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() >= 19 && b[4] == b':' && b[7] == b':' && b[10] == b' ' {
        Some(format!(
            "{}-{}-{}T{}",
            &s[0..4],
            &s[5..7],
            &s[8..10],
            &s[11..19]
        ))
    } else {
        None
    }
}

impl Dataset for TiffDataset {
    fn write_state(&self) -> Option<openreadout_core::live::WriteState> {
        self.growing_state()
    }

    fn attachments(&self) -> Result<Vec<openreadout_core::model::AttachmentInfo>> {
        Ok(self
            .embedded
            .iter()
            .enumerate()
            .map(
                |(i, (name, bytes))| openreadout_core::model::AttachmentInfo {
                    index: i as u32,
                    name: name.clone(),
                    content_type: "JPG".into(),
                    extension: "jpg".into(),
                    offset: None,
                    size: bytes.len() as u64,
                    extra: BTreeMap::new(),
                },
            )
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        self.embedded
            .get(index as usize)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| {
                Error::Usage(format!(
                    "attachment {index} does not exist ({} attachments)",
                    self.embedded.len()
                ))
            })
    }

    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        // The first stored page of each image (at most 64 images: plates repeat themselves).
        let layouts: Vec<PageLayout> = self
            .series
            .iter()
            .take(64)
            .filter_map(|s| {
                s.planes.iter().flatten().find_map(|p| match p {
                    PlaneSrc::Page { file, page, .. } => self.layout(*file, *page).ok(),
                    PlaneSrc::Contiguous { file, .. } => self.layout(*file, 0).ok(),
                    PlaneSrc::Member { .. } => None,
                })
            })
            .collect();
        crate::assurance::internal(layouts.iter())
    }

    fn member_files(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|f| f.path.clone())
            .filter(|p| *p != self.path && self.fs.is_file(p))
            .collect()
    }
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self.series.iter().map(|s| s.info.clone()).collect();
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let mut notes = vec![format!("TIFF sub-format: {}", self.flavor.id())];
        notes.extend(self.notes.iter().cloned());
        let version = self.opened(0).map(|t| {
            if t.header.big_tiff {
                "6.0+BigTIFF".to_string()
            } else {
                "6.0".to_string()
            }
        });
        let format_version = match (&version, self.flavor) {
            (Some(v), Flavor::OmeTiff) => Some(format!(
                "{v}; OME-XML {}",
                self.ome_schema().unwrap_or_else(|| "unknown schema".into())
            )),
            (None, Flavor::OmeCompanion) => Some(format!(
                "OME-XML {}",
                self.ome_schema().unwrap_or_else(|| "unknown schema".into())
            )),
            (None, Flavor::MetamorphNd) => self
                .nd
                .as_ref()
                .map(|nd| format!("MetaMorph ND {}", nd.version.as_deref().unwrap_or("?"))),
            (v, _) => v.clone(),
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: TiffReader.descriptor(),
            format_version,
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let ome = self
            .ome_xml
            .as_ref()
            .and_then(|(_, _, xml)| xml_to_json(xml.trim_start_matches('\u{feff}')));
        let (Value::Object(base), Some(ome)) = (&self.vendor, ome) else {
            return Ok(self.vendor.clone());
        };
        // Key order: "tiff", "ome", then the flavour's own keys.
        let mut out = Map::new();
        if let Some(t) = base.get("tiff") {
            out.insert("tiff".into(), t.clone());
        }
        out.insert("ome".into(), ome);
        for (k, v) in base.iter().filter(|(k, _)| k.as_str() != "tiff") {
            out.insert(k.clone(), v.clone());
        }
        Ok(Value::Object(out))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = self.provenance.clone();
        p.entry("images[].pixel_type".into())
            .or_insert(Source::Spec);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self.list_entries())
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        self.read_stored_plane(image, idx)
    }

    fn read_plane_level(&mut self, image: u32, idx: PlaneIndex, level: u32) -> Result<Plane> {
        if level == 0 {
            return self.read_plane(image, idx);
        }
        let (w, h) = self.level_dims(image, level)?;
        self.read_level_region(image, idx, level, Region::full(w, h))
    }

    fn read_region(
        &mut self,
        image: u32,
        idx: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        self.read_level_region(image, idx, level, region)
    }

    fn check(&mut self) -> Result<CheckReport> {
        Ok(crate::check::run(self))
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        if image == 0 && self.flavor == Flavor::MetamorphStk {
            return Ok(self.stk_frames(limit));
        }
        if image == 0 && self.flavor == Flavor::Eer {
            return Ok(self.eer_frames(limit));
        }
        Ok((0, Vec::new()))
    }
}

/// The directory a data set's sibling files are looked up in (`.` for a bare file name).
pub(crate) fn dir_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}
