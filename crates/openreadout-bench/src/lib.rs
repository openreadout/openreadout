//! Shared helpers for the benchmark suite (`benches/suite.rs`) and the memory-ceiling tests.
//!
//! Two kinds of inputs:
//! - **Corpus files** (smoke tier), found under `$OPENREADOUT_CORPUS_DIR` or `corpus/files`.
//!   A benchmark whose file is absent is skipped with a note on stderr.
//! - **Synthetic inputs** generated here, so `cargo bench` measures something useful on a
//!   machine without the corpus: an in-memory image [`Dataset`], an OME-TIFF written from it,
//!   an indexed mzML written by our own writer, a SpikeGLX stream, and encoded codec payloads.
//!
//! Nothing here is part of the shipped binary.
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry, SpectraInfo, Spectrum,
};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex, Registry};
use openreadout_core::{PixelType, Plane, ProvenanceMap, Result};

/// The readers the suite exercises, in the CLI's detection order (a subset of the CLI registry).
pub fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_spectro::OpusReader))
        .with(Box::new(openreadout_spectro::OmnicReader))
        .with(Box::new(openreadout_spectro::WdfReader))
        .with(Box::new(openreadout_spectro::PeSpReader))
        .with(Box::new(openreadout_spectro::JwsReader))
        .with(Box::new(openreadout_spectro::CaryReader))
        .with(Box::new(openreadout_biophys::ItcReader))
        .with(Box::new(openreadout_biophys::BiacoreReader))
        .with(Box::new(openreadout_biophys::BiacoreEvaluationReader))
        .with(Box::new(openreadout_biophys::SeahorseReader))
        .with(Box::new(openreadout_biophys::OctetReader))
        .with(Box::new(openreadout_biophys::ZetasizerReader))
        .with(Box::new(openreadout_biophys::GprReader))
        .with(Box::new(openreadout_gel::ImageLabReader))
        .with(Box::new(openreadout_fplc::UnicornResReader))
        .with(Box::new(openreadout_fplc::UnicornZipReader))
        .with(Box::new(openreadout_epr::Bes3tReader))
        .with(Box::new(openreadout_epr::EspReader))
        .with(Box::new(openreadout_xrd::XrdmlReader))
        .with(Box::new(openreadout_xrd::BrukerRawReader))
        .with(Box::new(openreadout_xrd::BrmlReader))
        .with(Box::new(openreadout_xrd::RasReader))
        .with(Box::new(openreadout_xrd::RasxReader))
        .with(Box::new(openreadout_echem::MprReader))
        .with(Box::new(openreadout_echem::MptReader))
        .with(Box::new(openreadout_echem::GamryReader))
        .with(Box::new(openreadout_echem::NdaReader))
        .with(Box::new(openreadout_echem::NdaxReader))
        .with(Box::new(openreadout_echem::ArbinReader))
        .with(Box::new(openreadout_thermal::NgbReader))
        .with(Box::new(openreadout_thermal::TaReader))
        .with(Box::new(openreadout_thermal::TriosReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_fcs::FcsReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_mzml::MzmlbReader))
        .with(Box::new(openreadout_oir::OirReader))
        .with(Box::new(openreadout_vsi::VsiReader))
        .with(Box::new(openreadout_wsi::MiraxReader))
        .with(Box::new(openreadout_zvi::ZviReader))
        .with(Box::new(openreadout_dcimg::DcimgReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_spikeglx::SpikeGlxReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_hdf5::ImsReader))
}

/// The corpus directory: `$OPENREADOUT_CORPUS_DIR`, else `corpus/files` at the workspace root.
pub fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

/// A corpus file by its name under the corpus directory, if it has been fetched.
pub fn corpus_file(name: &str) -> Option<PathBuf> {
    let p = corpus_dir().join(name);
    p.exists().then_some(p)
}

/// Deterministic pseudo-random bytes (xorshift), for payloads that must not compress to nothing.
pub fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        out.extend_from_slice(&s.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A microscopy-like 16-bit plane: a smooth gradient plus a little noise, little-endian.
/// Compresses about as well as real fluorescence data (2-4x with deflate), unlike pure noise.
pub fn gradient_u16(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let n = width as usize * height as usize;
    let jitter = noise(n, seed);
    let mut out = Vec::with_capacity(n * 2);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let base = ((x * 7 + y * 3) % 4096) as u16 + 200;
            let v = base + u16::from(jitter[y * width as usize + x] & 0x0F);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Low byte half followed by high byte half: the inverse of
/// [`openreadout_codecs::unshuffle_hilo`], used to build zstd1+HiLo payloads.
pub fn shuffle_hilo(interleaved: &[u8]) -> Vec<u8> {
    let n = interleaved.len() / 2;
    let mut out = Vec::with_capacity(interleaved.len());
    out.extend(interleaved[..n * 2].iter().step_by(2));
    out.extend(interleaved[1..n * 2].iter().step_by(2));
    if interleaved.len() % 2 == 1 {
        out.push(interleaved[interleaved.len() - 1]);
    }
    out
}

/// Encoded payloads for the codec micro-benchmarks, all decoding to `raw`.
#[derive(Debug, Clone)]
pub struct CodecInputs {
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
    /// Little-endian 16-bit samples.
    pub raw: Vec<u8>,
    /// Plain zstd frame.
    pub zstd: Vec<u8>,
    /// CZI-style "zstd1" payload: 3-byte header with the HiLo flag, then a zstd frame of the
    /// byte-shuffled samples.
    pub zstd1_hilo: Vec<u8>,
    /// TIFF-flavoured LZW (MSB first, early change).
    pub lzw: Vec<u8>,
    /// zlib stream.
    pub zlib: Vec<u8>,
    /// Baseline 8-bit RGB JPEG (quality 90) of a `width` x `height` image.
    pub jpeg: Vec<u8>,
    /// Lossless JPEG 2000 codestream of the 16-bit samples (14 significant bits).
    pub jpeg2000: Vec<u8>,
}

impl CodecInputs {
    /// Build every payload from one `width` x `height` gradient plane.
    pub fn new(width: u32, height: u32) -> Self {
        let raw = gradient_u16(width, height, 7);
        let zstd = ruzstd::encoding::compress_to_vec(
            raw.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let mut zstd1_hilo = vec![3u8, 1, 1];
        zstd1_hilo.extend(ruzstd::encoding::compress_to_vec(
            shuffle_hilo(&raw).as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
        let lzw = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&raw)
            .expect("LZW encoding of an in-memory buffer");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw).expect("zlib encoding to a Vec");
        let zlib = z.finish().expect("zlib encoding to a Vec");
        let rgb: Vec<u8> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| {
                let v = (u16::from_le_bytes([c[0], c[1]]) >> 4) as u8;
                [v, v.wrapping_add(40), v.wrapping_mul(3)]
            })
            .collect();
        let canvas = openreadout_preview::Canvas { width, height, rgb };
        let jpeg = openreadout_preview::jpeg::encode(&canvas, 90);
        let jpeg2000 = dicom_toolkit_jpeg2000::encode(
            &raw,
            width,
            height,
            1,
            14,
            false,
            &dicom_toolkit_jpeg2000::EncodeOptions::default(),
        )
        .expect("JPEG 2000 encoding of an in-memory plane");
        CodecInputs {
            width,
            height,
            raw,
            zstd,
            zstd1_hilo,
            lzw,
            zlib,
            jpeg,
            jpeg2000,
        }
    }
}

/// An in-memory image file: `size_c` x `size_z` x `size_t` planes of 16-bit gradients.
/// Planes are generated on request, so a "multi-GB" dataset costs no memory until read.
#[derive(Debug, Clone)]
pub struct SyntheticImage {
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
    /// Channels.
    pub size_c: u32,
    /// Z planes.
    pub size_z: u32,
    /// Time points.
    pub size_t: u32,
    /// One precomputed plane, returned (copied) for every index with its first sample
    /// replaced by the plane number, so plane hashes differ.
    template: std::sync::Arc<Vec<u8>>,
}

impl SyntheticImage {
    /// An image of `size_c` x `size_z` x `size_t` planes of `width` x `height`.
    pub fn new(width: u32, height: u32, size_c: u32, size_z: u32, size_t: u32) -> Self {
        SyntheticImage {
            width,
            height,
            size_c,
            size_z,
            size_t,
            template: std::sync::Arc::new(gradient_u16(width, height, 11)),
        }
    }

    /// Bytes of one decoded plane.
    pub fn plane_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 2
    }

    /// Bytes of all planes.
    pub fn total_bytes(&self) -> u64 {
        self.plane_bytes() * u64::from(self.size_c * self.size_z * self.size_t)
    }

    fn descriptor() -> FormatDescriptor {
        let mut d = openreadout_tiff::TiffReader.descriptor();
        d.id = "synthetic".into();
        d.name = "Synthetic benchmark image".into();
        d
    }
}

impl Dataset for SyntheticImage {
    fn info(&self) -> Result<FileInfo> {
        let mut im = ImageInfo::new(0, self.width, self.height, PixelType::Uint16);
        im.size_c = self.size_c;
        im.size_z = self.size_z;
        im.size_t = self.size_t;
        let im = im.finish();
        Ok(FileInfo {
            path: "synthetic".into(),
            size_bytes: self.total_bytes(),
            format: Self::descriptor(),
            format_version: None,
            plane_count: im.plane_count,
            images: vec![im],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes: Vec::new(),
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }
    fn read_plane(&mut self, _image: u32, i: PlaneIndex) -> Result<Plane> {
        let mut data = self.template.as_ref().clone();
        let n = (i.t * self.size_z + i.z) * self.size_c + i.c;
        data[..2].copy_from_slice(&(n as u16).to_le_bytes());
        Ok(Plane {
            width: self.width,
            height: self.height,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data,
        })
    }
    /// Copies only the region's rows, so a streaming export never holds a whole plane.
    fn read_region(
        &mut self,
        image: u32,
        i: PlaneIndex,
        level: u32,
        region: openreadout_core::region::Region,
    ) -> Result<Plane> {
        if level > 0 {
            let plane = self.read_plane_level(image, i, level)?;
            return openreadout_core::region::crop(plane, region, "synthetic");
        }
        region.check_within(self.width, self.height, "synthetic")?;
        let row = self.width as usize * 2;
        let (x, w) = (region.x as usize * 2, region.width as usize * 2);
        let mut data = Vec::with_capacity(w * region.height as usize);
        for y in region.y..region.y + region.height {
            let start = y as usize * row + x;
            data.extend_from_slice(&self.template[start..start + w]);
        }
        if region.x == 0 && region.y == 0 {
            let n = (i.t * self.size_z + i.z) * self.size_c + i.c;
            data[..2].copy_from_slice(&(n as u16).to_le_bytes());
        }
        Ok(Plane {
            width: region.width,
            height: region.height,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data,
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synthetic", "synthetic"))
    }
}

/// OME-TIFF export options: every plane, `codec`, overwriting the output.
pub fn tiff_options(codec: openreadout_ometiff::Codec) -> openreadout_ometiff::ExportOptions {
    let mut o = openreadout_ometiff::ExportOptions::default();
    o.codec = codec;
    o.overwrite = true;
    o
}

/// OME-Zarr export options: every plane, `codec`, 1024-pixel chunks, `levels` resolution
/// levels (`None` = the default pyramid), overwriting the output.
pub fn zarr_options(
    codec: openreadout_ometiff::Codec,
    levels: Option<u32>,
) -> openreadout_omezarr::ZarrExportOptions {
    let mut o = openreadout_omezarr::ZarrExportOptions::default();
    o.codec = codec;
    o.overwrite = true;
    o.chunk = 1024;
    o.levels = levels;
    o
}

/// Write `ds` as an OME-TIFF at `path` (the synthetic input for `info` and plane
/// decode benchmarks of the TIFF family).
pub fn write_ome_tiff(
    ds: &mut dyn Dataset,
    path: &Path,
    codec: openreadout_ometiff::Codec,
) -> Result<()> {
    openreadout_ometiff::export_ome_tiff(ds, Path::new("synthetic"), path, &tiff_options(codec))
        .map(|_| ())
}

/// A synthetic LC-MS run: `scans` profile spectra of `points` points each.
#[derive(Debug, Clone)]
pub struct SyntheticSpectra {
    /// Spectra in the run.
    pub scans: u64,
    /// Points per spectrum.
    pub points: usize,
}

impl Dataset for SyntheticSpectra {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: "synthetic".into(),
            size_bytes: 0,
            format: SyntheticImage::descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![SpectraInfo {
                index: 0,
                scan_count: self.scans,
                ms_levels: vec![1],
                rt_range_s: Some([0.0, self.scans as f64]),
                ..SpectraInfo::default()
            }],
            traces: Vec::new(),
            plane_count: 0,
            notes: Vec::new(),
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Err(openreadout_core::Error::Other("no images".into()))
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synthetic", "synthetic"))
    }
    fn read_spectrum(&mut self, _index: u32, spectrum: u64) -> Result<Spectrum> {
        let jitter = noise(self.points * 4, spectrum);
        let mz: Vec<f64> = (0..self.points)
            .map(|i| 100.0 + i as f64 * 0.01 + f64::from(jitter[i]) * 1e-6)
            .collect();
        let intensity: Vec<f32> = jitter
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from(u16::from_le_bytes([c[0], c[1]])))
            .collect();
        Ok(Spectrum {
            index: spectrum,
            scan_number: spectrum + 1,
            ms_level: 1,
            rt_s: Some(spectrum as f64),
            polarity: "positive".into(),
            centroided: false,
            native_id: Some(format!("scan={}", spectrum + 1)),
            extra: BTreeMap::new(),
            mz,
            intensity,
            ..Spectrum::default()
        })
    }
}

/// Write an indexed mzML (zlib arrays) with our own writer: the synthetic mzML parse input.
pub fn write_mzml(scans: u64, points: usize, path: &Path) -> Result<()> {
    let mut ds = SyntheticSpectra { scans, points };
    openreadout_mzml_writer::export_mzml(&mut ds, Path::new("synthetic.raw"), path, &{
        let mut o = openreadout_mzml_writer::MzmlExportOptions::default();
        o.overwrite = true;
        o
    })
    .map(|_| ())
}

/// Write a SpikeGLX NI-DAQ stream (`<stem>.nidq.bin` + `.meta`) of `channels` analog channels
/// of `samples` 16-bit samples at 30 kHz. Returns the `.bin` path.
pub fn write_spikeglx(dir: &Path, channels: u32, samples: u64) -> std::io::Result<PathBuf> {
    let bin = dir.join("synthetic_g0_t0.nidq.bin");
    let n = usize::try_from(samples * u64::from(channels)).unwrap_or(usize::MAX);
    let data = noise(n * 2, 3);
    std::fs::write(&bin, &data)?;
    let meta = format!(
        "acqMnMaXaDw=0,0,{channels},0\nappVersion=20230323\nfileSizeBytes={}\nfileTimeSecs={}\nfirstSample=0\n\
         nSavedChans={channels}\nniAiRangeMax=5\nniAiRangeMin=-5\nniMAGain=1\nniMNGain=1\n\
         niMaxInt=32768\nniSampRate=30000\nniXAChans1=0:{}\nniXDBytes1=0\n\
         snsMnMaXaDw=0,0,{channels},0\nsnsSaveChanSubset=all\ntypeThis=nidq\n",
        data.len(),
        samples as f64 / 30000.0,
        channels - 1,
    );
    std::fs::write(bin.with_extension("meta"), meta)?;
    Ok(bin)
}
