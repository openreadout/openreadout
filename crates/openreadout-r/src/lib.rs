//! Native half of the `openreadout` R package (`r/openreadout`), built with extendr.
//!
//! Every function here returns either its result or, on failure, a list of class
//! `openreadout_native_error` (`message`, `code`, `exit_code`, `hint`, `class`); the R wrappers
//! (`R/native.R`) turn that into a classed R condition. No function raises an R error itself, so
//! no R `longjmp` ever crosses Rust frames, and argument checking happens in R before the call.
//!
//! Results are R vectors built directly (pixels, table columns, spectra) or JSON strings the R
//! side parses with jsonlite (metadata, reports, analysis outputs), so the field names are those of
//! `openreadout <command> --json`.
//!
//! This crate contains no `unsafe` code of its own; the extendr macros generate the `extern "C"`
//! entry points (see the `no_unsafe_in_source` test).

use std::fmt;
use std::path::Path;

use extendr_api::prelude::*;
use openreadout_core::model::DetectOutput;
use openreadout_core::reader::PlaneIndex;
use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error as CoreError, PixelType, Region, Registry};

mod columnar;
mod export;

/// Every reader, in detection priority order: the same list as the CLI
/// (`crates/openreadout-cli/src/registry.rs`; the `registry_matches_cli` test keeps them equal).
pub fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_fcs::FcsReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_mzml::MzmlbReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
        .with(Box::new(openreadout_oir::OirReader))
        .with(Box::new(openreadout_vsi::VsiReader))
        .with(Box::new(openreadout_wsi::MiraxReader))
        .with(Box::new(openreadout_zvi::ZviReader))
        .with(Box::new(openreadout_oif::OibReader))
        .with(Box::new(openreadout_oif::OifReader))
        .with(Box::new(openreadout_dcimg::DcimgReader))
        .with(Box::new(openreadout_qpcr::RdmlReader))
        .with(Box::new(openreadout_qpcr::EdsReader))
        .with(Box::new(openreadout_qpcr::PcrdReader))
        .with(Box::new(openreadout_qpcr::RexReader))
        .with(Box::new(openreadout_qpcr::IxoReader))
        .with(Box::new(openreadout_qpcr::ExportReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_abf::AtfReader))
        .with(Box::new(openreadout_neuralynx::NeuralynxReader))
        .with(Box::new(openreadout_blackrock::BlackrockReader))
        .with(Box::new(openreadout_spikeglx::SpikeGlxReader))
        .with(Box::new(openreadout_intan::IntanReader))
        .with(Box::new(openreadout_plexon::PlexonReader))
        .with(Box::new(openreadout_ephys::HekaReader))
        .with(Box::new(openreadout_ephys::Spike2Reader))
        .with(Box::new(openreadout_ephys::WinWcpReader))
        .with(Box::new(openreadout_ephys::OpenEphysReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::OpenLabReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
        .with(Box::new(openreadout_hcs::HarmonyReader))
        .with(Box::new(openreadout_hcs::ImageXpressReader))
        .with(Box::new(openreadout_hcs::CellVoyagerReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_em::MrcReader))
        .with(Box::new(openreadout_em::DmReader))
        .with(Box::new(openreadout_em::SerReader))
        .with(Box::new(openreadout_em::EmdReader))
        .with(Box::new(openreadout_hdf5::ImsReader))
        .with(Box::new(openreadout_hdf5::NwbReader))
        .with(Box::new(openreadout_nmr::BrukerReader))
        .with(Box::new(openreadout_nmr::JcampReader))
        .with(Box::new(openreadout_nmr::VarianReader))
        .with(Box::new(openreadout_nmr::JeolReader))
        .with(Box::new(openreadout_nmr::SpinsolveReader))
        .with(Box::new(openreadout_spectro::OpusReader))
        .with(Box::new(openreadout_spectro::OmnicReader))
        .with(Box::new(openreadout_spectro::WdfReader))
        .with(Box::new(openreadout_spectro::PeSpReader))
        .with(Box::new(openreadout_spectro::JwsReader))
        .with(Box::new(openreadout_spectro::SpcReader))
        .with(Box::new(openreadout_spectro::WitecReader))
        .with(Box::new(openreadout_spectro::AgilentFpaReader))
        .with(Box::new(openreadout_spectro::FsmReader))
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
        // loose: any other HDF5 file (after EMD, Imaris and NWB, which claim theirs definitely)
        .with(Box::new(openreadout_hdf5::Hdf5Reader))
        // last: text/CSV/XLSX exports; the generic plate-matrix fallback is the loosest sniff
        .with(Box::new(openreadout_plate::PlateReader))
}

// ---------------------------------------------------------------------------------------------
// Errors

/// The R condition class (after `openreadout_error`) of each core error.
fn error_class(e: &CoreError) -> &'static str {
    match e {
        CoreError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            "openreadout_file_not_found"
        }
        CoreError::Io { .. } => "openreadout_io_error",
        CoreError::UnknownFormat { .. } => "openreadout_unknown_format",
        CoreError::Usage(_) => "openreadout_usage_error",
        CoreError::Corrupt { .. } => "openreadout_corrupt_file",
        CoreError::Unsupported { .. } => "openreadout_unsupported_feature",
        _ => "openreadout_error",
    }
}

/// A core error as the list the R side turns into a condition.
fn r_error(e: &CoreError) -> Robj {
    let mut l = list!(
        message = e.to_string(),
        code = e.code(),
        exit_code = e.exit_code(),
        hint = e.hint().unwrap_or_default(),
        class = error_class(e)
    );
    l.set_class(["openreadout_native_error"])
        .map_or_else(|_| Robj::from(()), |l| l.clone().into())
}

/// A usage error (bad arguments that got past the R-side checks).
fn usage(msg: impl Into<String>) -> Robj {
    r_error(&CoreError::Usage(msg.into()))
}

/// `Ok` value or the error list.
fn done<T: Into<Robj>>(r: openreadout_core::Result<T>) -> Robj {
    match r {
        Ok(v) => v.into(),
        Err(e) => r_error(&e),
    }
}

/// A serializable value as a JSON string, or the error list.
fn json<T: serde::Serialize>(r: openreadout_core::Result<T>) -> Robj {
    done(r.and_then(|v| {
        serde_json::to_string(&v)
            .map_err(|e| CoreError::Other(format!("JSON serialization failed: {e}")))
    }))
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, s: &str) -> openreadout_core::Result<T> {
    serde_json::from_str(s).map_err(|e| CoreError::Usage(format!("{what}: {e}")))
}

// ---------------------------------------------------------------------------------------------
// Open files

/// An opened file, held by R as an external pointer (class `openreadout_handle`); dropped when
/// R garbage-collects it or `openreadout_close()` is called.
pub struct Handle {
    path: String,
    format: String,
    ds: Option<Box<dyn Dataset>>,
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("path", &self.path)
            .field("format", &self.format)
            .field("open", &self.ds.is_some())
            .finish()
    }
}

/// Run `f` on the open dataset behind an R external pointer.
fn with_ds<T>(
    h: &Robj,
    f: impl FnOnce(&mut dyn Dataset, &str) -> openreadout_core::Result<T>,
) -> openreadout_core::Result<T> {
    let mut ptr: ExternalPtr<Handle> = ExternalPtr::try_from(h)
        .map_err(|_| CoreError::Usage("expected a file opened with openreadout_open()".into()))?;
    let handle = ptr.try_addr_mut().map_err(|_| {
        CoreError::Usage("this file handle is no longer valid (saved and restored?): open the file again with openreadout_open()".into())
    })?;
    let path = handle.path.clone();
    let ds = handle.ds.as_mut().ok_or_else(|| {
        CoreError::Usage(format!(
            "{path} was closed with openreadout_close(); open it again with openreadout_open()"
        ))
    })?;
    f(ds.as_mut(), &path)
}

/// Open a file (format detected from its content): an external pointer, or the error list.
#[extendr]
fn rs_open(path: &str) -> Robj {
    match registry().open_input(&Input::local(path)) {
        Ok((det, ds)) => {
            let h = Handle {
                path: path.to_string(),
                format: det.format_id.to_string(),
                ds: Some(ds),
            };
            let mut ptr = ExternalPtr::new(h);
            match ptr.set_class(["openreadout_handle"]) {
                Ok(_) => ptr.into(),
                Err(e) => usage(e.to_string()),
            }
        }
        Err(e) => r_error(&e),
    }
}

/// Release the file (idempotent).
#[extendr]
fn rs_close(h: Robj) -> Robj {
    if let Ok(mut ptr) = ExternalPtr::<Handle>::try_from(&h)
        && let Ok(handle) = ptr.try_addr_mut()
    {
        handle.ds = None;
    }
    Robj::from(())
}

/// `c(path, format, open)` of a handle.
#[extendr]
fn rs_handle_state(h: Robj) -> Robj {
    match ExternalPtr::<Handle>::try_from(&h) {
        Ok(ptr) => match ptr.try_addr() {
            Ok(handle) => list!(
                path = handle.path.clone(),
                format = handle.format.clone(),
                open = handle.ds.is_some()
            )
            .into(),
            Err(_) => list!(path = "", format = "", open = false).into(),
        },
        Err(_) => usage("expected a file opened with openreadout_open()"),
    }
}

// ---------------------------------------------------------------------------------------------
// Metadata

#[extendr]
fn rs_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `openreadout self formats --json` data.
#[extendr]
fn rs_formats() -> Robj {
    json(Ok(openreadout_core::model::FormatsOutput {
        formats: registry().descriptors(),
    }))
}

/// `openreadout info --view format --json` data.
#[extendr]
fn rs_detect(path: &str) -> Robj {
    let reg = registry();
    json(reg.detect(Path::new(path)).map(|(r, d)| DetectOutput {
        path: path.to_string(),
        format: d.format_id.to_string(),
        name: r.descriptor().name,
        confidence: d.confidence,
        note: d.note,
    }))
}

/// `openreadout info --json` data (`FileInfo` plus the `experiment`).
#[extendr]
fn rs_info(h: Robj) -> Robj {
    json(with_ds(&h, |ds, _| {
        let info = ds.info()?;
        Ok(openreadout_core::InfoOutput::new(ds, info))
    }))
}

/// `openreadout info --view VIEW --json` data for the views `full` (the vendor tree only with
/// `vendor`), `structure` and `explain` (answering `ask` when it is not empty).
#[extendr]
fn rs_info_view(h: Robj, view: &str, ask: &str, vendor: bool) -> Robj {
    let view = view.to_string();
    let ask = (!ask.is_empty()).then(|| ask.to_string());
    json(with_ds(
        &h,
        |ds, path| -> openreadout_core::Result<serde_json::Value> {
            let value = match view.as_str() {
                "full" => {
                    let mut info = ds.info()?;
                    openreadout_core::reader::attach_frames(
                        ds,
                        &mut info,
                        Some(openreadout_core::reader::DEFAULT_FRAME_RECORDS),
                    )?;
                    serde_json::to_value(openreadout_core::model::Dump {
                        file: openreadout_core::InfoOutput::new(ds, info),
                        vendor: if vendor {
                            Some(ds.vendor_metadata()?)
                        } else {
                            None
                        },
                        provenance: ds.provenance(),
                    })
                }
                "structure" => serde_json::to_value(openreadout_core::model::Listing {
                    path: path.to_string(),
                    format: ds.info()?.format.id,
                    entries: ds.entries()?,
                    acquisition: None,
                }),
                "explain" => {
                    let mut info = ds.info()?;
                    let experiment = openreadout_core::experiment::of_dataset(ds, &info);
                    let _ = openreadout_core::reader::attach_frames(ds, &mut info, Some(1));
                    let assurance = openreadout_core::assurance::assess_dataset(ds, &info);
                    serde_json::to_value(
                        openreadout_ops::explain::explain_with(
                            &info,
                            Some(&experiment),
                            ask.as_deref(),
                        )
                        .with_assurance(assurance),
                    )
                }
                other => {
                    return Err(CoreError::Usage(format!(
                        "view must be summary, full, structure, explain or format, not {other:?}"
                    )));
                }
            };
            value.map_err(|e| CoreError::Other(format!("JSON serialization failed: {e}")))
        },
    ))
}

/// Pixel statistics (`openreadout stats --json` data): per plane (with `per_plane`), per image
/// and channel, and per image; `image` < 0 means every image.
#[extendr]
fn rs_stats(h: Robj, image: i32, select: Vec<String>, per_plane: bool) -> Robj {
    json(with_ds(&h, |ds, _| {
        let info = ds.info()?;
        let mut req = openreadout_core::stats::StatsRequest::default();
        req.image = u32::try_from(image).ok();
        req.select = select;
        req.per_plane = per_plane;
        openreadout_core::stats::compute_stats(
            ds,
            &info,
            &req,
            &openreadout_core::parallel::ReadContext::default(),
        )
    }))
}

/// `openreadout check --json` data.
#[extendr]
fn rs_check(h: Robj) -> Robj {
    json(with_ds(&h, |ds, _| ds.check()))
}

/// OME-XML (2016-06, `MetadataOnly` pixels) describing every image.
#[extendr]
fn rs_ome_xml(h: Robj) -> Robj {
    done(with_ds(&h, |ds, _| {
        let info = ds.info()?;
        let images: Vec<openreadout_ometiff::WrittenImage<'_>> = info
            .images
            .iter()
            .map(openreadout_ometiff::WrittenImage::whole)
            .collect();
        let creator = concat!("openreadout ", env!("CARGO_PKG_VERSION"));
        openreadout_ometiff::build_ome_xml_metadata_only(&info, &images, creator, None)
            .map_err(CoreError::Other)
    }))
}

// ---------------------------------------------------------------------------------------------
// Pixels

/// R storage for a pixel type: integer (exact for every type up to 32-bit signed) or double
/// (uint32 and floating point).
fn stored_as_integer(t: PixelType) -> bool {
    matches!(
        t,
        PixelType::Int8
            | PixelType::Int16
            | PixelType::Int32
            | PixelType::Uint8
            | PixelType::Uint16
    )
}

/// The pixel types this crate converts (`PixelType` is non-exhaustive).
fn known_pixel_type(t: PixelType) -> bool {
    matches!(
        t,
        PixelType::Int8
            | PixelType::Int16
            | PixelType::Int32
            | PixelType::Uint8
            | PixelType::Uint16
            | PixelType::Uint32
            | PixelType::Float
            | PixelType::Double
    )
}

/// Convert the little-endian samples of `src` (`N` bytes each, `spp` interleaved per pixel) into
/// `dst`, sample-major (all of sample 0, then sample 1, ...): R's column-major `(x, y, s)`.
/// One tight loop per type: this runs over every pixel read.
fn convert<T: Copy, const N: usize>(
    dst: &mut [T],
    src: &[u8],
    spp: usize,
    npix: usize,
    f: impl Fn([u8; N]) -> T,
) {
    let (words, _) = src.as_chunks::<N>();
    if spp == 1 {
        for (d, w) in dst.iter_mut().zip(words) {
            *d = f(*w);
        }
    } else {
        for (i, px) in words.chunks_exact(spp).take(npix).enumerate() {
            for (s, w) in px.iter().enumerate() {
                if let Some(d) = dst.get_mut(s * npix + i) {
                    *d = f(*w);
                }
            }
        }
    }
}

/// A plane's samples as R integers (types up to 32-bit signed).
fn to_integers(t: PixelType, dst: &mut [Rint], src: &[u8], spp: usize, npix: usize) {
    match t {
        PixelType::Int8 => convert(dst, src, spp, npix, |b: [u8; 1]| {
            Rint::from(i32::from(i8::from_le_bytes(b)))
        }),
        PixelType::Uint8 => convert(dst, src, spp, npix, |b: [u8; 1]| {
            Rint::from(i32::from(b[0]))
        }),
        PixelType::Int16 => convert(dst, src, spp, npix, |b: [u8; 2]| {
            Rint::from(i32::from(i16::from_le_bytes(b)))
        }),
        PixelType::Uint16 => convert(dst, src, spp, npix, |b: [u8; 2]| {
            Rint::from(i32::from(u16::from_le_bytes(b)))
        }),
        PixelType::Int32 => convert(dst, src, spp, npix, |b: [u8; 4]| {
            Rint::from(i32::from_le_bytes(b))
        }),
        // not reached: the caller stores the other types as double
        _ => {}
    }
}

/// A plane's samples as R doubles (unsigned 32-bit and floating point).
fn to_doubles(t: PixelType, dst: &mut [Rfloat], src: &[u8], spp: usize, npix: usize) {
    match t {
        PixelType::Uint32 => convert(dst, src, spp, npix, |b: [u8; 4]| {
            Rfloat::from(f64::from(u32::from_le_bytes(b)))
        }),
        PixelType::Float => convert(dst, src, spp, npix, |b: [u8; 4]| {
            Rfloat::from(f64::from(f32::from_le_bytes(b)))
        }),
        PixelType::Double => convert(dst, src, spp, npix, |b: [u8; 8]| {
            Rfloat::from(f64::from_le_bytes(b))
        }),
        // not reached: the caller stores the other types as integers
        _ => {}
    }
}

/// Read the planes `(c[i], z[i], t[i])` (zero-based) of `image` at pyramid `level`, optionally the
/// rectangle `region = c(x, y, width, height)` (zero-based offsets), into one R vector, plane after
/// plane. Within a plane, samples are stored x fastest, then y, then sample (RGB de-interleaved),
/// which is R's column-major order for an array of dimensions `(x, y[, s])`.
///
/// Returns the vector itself, with attributes `openreadout_width`, `openreadout_height`, `openreadout_samples` and
/// `openreadout_pixel_type` (not a list holding it: R would then share the vector, and shaping it into an
/// array would copy every pixel), or the error list.
#[extendr]
fn rs_read_planes(
    h: Robj,
    image: i32,
    c: Vec<i32>,
    z: Vec<i32>,
    t: Vec<i32>,
    level: i32,
    region: Nullable<Vec<i32>>,
) -> Robj {
    if image < 0 || level < 0 || c.len() != z.len() || c.len() != t.len() {
        return usage(
            "image, level and the plane indices must be non-negative, c/z/t equally long",
        );
    }
    if c.iter().chain(&z).chain(&t).any(|&v| v < 0) {
        return usage("plane indices must be non-negative");
    }
    let region = match region {
        Nullable::NotNull(r) => {
            if r.len() != 4 || r.iter().any(|&v| v < 0) || r[2] == 0 || r[3] == 0 {
                return usage(
                    "region must be c(x, y, width, height) with a positive width and height",
                );
            }
            Some(Region::new(
                r[0] as u32,
                r[1] as u32,
                r[2] as u32,
                r[3] as u32,
            ))
        }
        Nullable::Null => None,
    };
    let image = image as u32;
    let level = level as u32;
    let planes: Vec<PlaneIndex> = c
        .iter()
        .zip(&z)
        .zip(&t)
        .map(|((&c, &z), &t)| PlaneIndex {
            c: c as u32,
            z: z as u32,
            t: t as u32,
        })
        .collect();
    let read = |ds: &mut dyn Dataset, idx: PlaneIndex| match region {
        Some(r) => ds.read_region(image, idx, level, r),
        None => ds.read_plane_level(image, idx, level),
    };
    let res = with_ds(&h, |ds, _| {
        let Some(&first_idx) = planes.first() else {
            return Err(CoreError::Usage("no planes selected".into()));
        };
        let first = read(ds, first_idx)?;
        let (w, hgt, spp, ptype) = (
            first.width,
            first.height,
            first.samples_per_pixel.max(1),
            first.pixel_type,
        );
        if !known_pixel_type(ptype) {
            return Err(CoreError::unsupported(
                "r",
                format!("pixel type {}", ptype.ome_name()),
                "Export the image with openreadout_export(to = \"ome-tiff\") and read it with another package.",
            ));
        }
        let per_plane = (w as usize)
            .checked_mul(hgt as usize)
            .and_then(|v| v.checked_mul(spp as usize))
            .ok_or_else(|| CoreError::Usage("plane too large".into()))?;
        let total = per_plane
            .checked_mul(planes.len())
            .ok_or_else(|| CoreError::Usage("selection too large".into()))?;
        let as_int = stored_as_integer(ptype);
        let mut ints = if as_int {
            Integers::new(total)
        } else {
            Integers::new(0)
        };
        let mut dbls = if as_int {
            Doubles::new(0)
        } else {
            Doubles::new(total)
        };
        let npix = (w as usize) * (hgt as usize);
        let mut fill = |k: usize, p: &openreadout_core::Plane| -> openreadout_core::Result<()> {
            if p.width != w
                || p.height != hgt
                || p.samples_per_pixel.max(1) != spp
                || p.pixel_type != ptype
            {
                return Err(CoreError::Other(format!(
                    "planes of image {image} differ in geometry or type ({}x{} vs {w}x{hgt})",
                    p.width, p.height
                )));
            }
            if p.data.len() != p.expected_len() {
                return Err(CoreError::Other(format!(
                    "plane has {} bytes, expected {}",
                    p.data.len(),
                    p.expected_len()
                )));
            }
            let base = k * per_plane;
            // Source: pixel-interleaved (i * spp + s); destination: s-major (s * npix + i).
            let spp = spp as usize;
            if as_int {
                to_integers(ptype, &mut ints[base..base + per_plane], &p.data, spp, npix);
            } else {
                to_doubles(ptype, &mut dbls[base..base + per_plane], &p.data, spp, npix);
            }
            Ok(())
        };
        fill(0, &first)?;
        drop(first);
        for (k, &idx) in planes.iter().enumerate().skip(1) {
            let p = read(ds, idx)?;
            fill(k, &p)?;
        }
        let mut data: Robj = if as_int { ints.into() } else { dbls.into() };
        data.set_attrib("openreadout_width", w as i32)
            .and_then(|d| d.set_attrib("openreadout_height", hgt as i32))
            .and_then(|d| d.set_attrib("openreadout_samples", spp as i32))
            .and_then(|d| d.set_attrib("openreadout_pixel_type", ptype.ome_name()))
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(data)
    });
    done(res)
}

// ---------------------------------------------------------------------------------------------
// Tables, traces, spectra

/// A table, a trace or the spectra of a run as columns (the columns of `export --format parquet`):
/// `list(columns = <named list of vectors>, meta = <JSON: per-column unit/label/dtype, schema
/// metadata>)`. `kind` is `table`, `trace`, `spectra` or `scans` (the per-scan summary).
#[extendr]
fn rs_columnar(
    h: Robj,
    kind: &str,
    index: i32,
    sweep: Nullable<i32>,
    first_row: f64,
    last_row: Nullable<f64>,
    centroid: bool,
    max_rows: Nullable<f64>,
) -> Robj {
    let sweep = match sweep {
        Nullable::NotNull(s) if s >= 0 => Some(s as u32),
        Nullable::NotNull(_) => return usage("sweep must be non-negative"),
        Nullable::Null => None,
    };
    let last = match last_row {
        Nullable::NotNull(v) if v >= 0.0 => Some(v as u64),
        Nullable::NotNull(_) => return usage("last row must be non-negative"),
        Nullable::Null => None,
    };
    let max_rows = match max_rows {
        Nullable::NotNull(v) if v.is_finite() && v >= 0.0 => Some(v as u64),
        _ => None,
    };
    if index < 0 || first_row < 0.0 {
        return usage("index and first row must be non-negative");
    }
    let index = index as u32;
    let select = match kind {
        "table" => openreadout_arrow::ColumnarSelection::Table(index),
        "trace" => openreadout_arrow::ColumnarSelection::Trace(index),
        "spectra" | "scans" => openreadout_arrow::ColumnarSelection::Spectra(index),
        other => return usage(format!("unknown kind {other:?}")),
    };
    let mut opts = openreadout_arrow::ColumnarOptions::default();
    opts.select = select;
    opts.sweep = sweep;
    opts.rows = if first_row > 0.0 || last.is_some() {
        Some((first_row as u64, last))
    } else {
        None
    };
    opts.centroid = centroid;
    let per_scan = kind == "scans";
    let res = with_ds(&h, |ds, path| {
        let data = openreadout_arrow::read_columnar(ds, Path::new(path), &opts, max_rows)?;
        if per_scan {
            let s = data
                .summary
                .ok_or_else(|| CoreError::Other("no per-scan summary for these spectra".into()))?;
            let schema = s.schema();
            columnar::to_r(&schema, std::slice::from_ref(&s))
        } else {
            columnar::to_r(&data.schema, &data.batches)
        }
    });
    done(res)
}

/// One mass spectrum by zero-based `index` in run `run`, or by the instrument's `scan` number:
/// `list(meta = <JSON: the Spectrum without its arrays>, mz, intensity)`.
#[extendr]
fn rs_spectrum(h: Robj, run: i32, number: f64, by_scan: bool, centroid: bool) -> Robj {
    if run < 0 || number < 0.0 || !number.is_finite() {
        return usage("run and spectrum number must be non-negative");
    }
    let view = if centroid {
        openreadout_core::SpectrumView::Centroid
    } else {
        openreadout_core::SpectrumView::Primary
    };
    let res = with_ds(&h, |ds, _| {
        let mut sp = if by_scan {
            openreadout_core::reader::spectrum_by_scan(ds, run as u32, number as u64, view)?
        } else {
            ds.read_spectrum_view(run as u32, number as u64, view)?
        };
        let mz = std::mem::take(&mut sp.mz);
        let it: Vec<f64> = std::mem::take(&mut sp.intensity)
            .into_iter()
            .map(f64::from)
            .collect();
        let meta = serde_json::to_string(&sp)
            .map_err(|e| CoreError::Other(format!("JSON serialization failed: {e}")))?;
        Ok(list!(meta = meta, mz = mz, intensity = it))
    });
    done(res)
}

/// Scan headers of run `run` without decoding peaks (`openreadout scans --json` data):
/// `filter` is a `ScanFilter` as JSON; `limit` < 0 lists every match.
#[extendr]
fn rs_scans(h: Robj, run: i32, filter: &str, offset: f64, limit: f64) -> Robj {
    if run < 0 || offset < 0.0 || !offset.is_finite() {
        return usage("run and offset must be non-negative");
    }
    let limit = if limit < 0.0 { u64::MAX } else { limit as u64 };
    json(
        parse::<openreadout_core::ScanFilter>("scan filter", filter).and_then(|f| {
            with_ds(&h, |ds, path| {
                let format = ds.info()?.format.id;
                openreadout_core::scans::scan_list(
                    ds,
                    path,
                    &format,
                    run as u32,
                    &f,
                    offset as u64,
                    limit,
                )
            })
        }),
    )
}

// ---------------------------------------------------------------------------------------------
// Analyses (JSON in, JSON out: the MCP tools' arguments and the CLI's --json data)

/// `openreadout analyze KIND` on an open file, for the kinds that read one data set
/// (`options`: the arguments of that kind's MCP tool, as JSON).
#[extendr]
fn rs_analyze_dataset(h: Robj, kind: &str, options: &str) -> Robj {
    json(
        openreadout_batch::analyze::AnalyzeKind::parse(kind).and_then(|kind| {
            let o: serde_json::Map<String, serde_json::Value> = parse("analysis options", options)?;
            with_ds(&h, |ds, _| {
                let info = ds.info()?;
                openreadout_batch::analyze::run_dataset(kind, &o, ds, &info, &Default::default())
            })
        }),
    )
}

/// `openreadout analyze KIND FILE` by path: `list(json, png)`, `png` the fitted curve of an
/// assay with `"plot": true` (a raw vector) or `NULL`.
#[extendr]
fn rs_analyze(path: &str, kind: &str, options: &str) -> Robj {
    let res = openreadout_batch::analyze::AnalyzeKind::parse(kind).and_then(|kind| {
        let args = openreadout_batch::analyze::AnalyzeArgs {
            file: path.to_string(),
            kind,
            options: parse("analysis options", options)?,
            strict: None,
        };
        let (out, png) = openreadout_batch::analyze::run(&registry(), &args)?;
        let text = serde_json::to_string(&out)
            .map_err(|e| CoreError::Other(format!("JSON serialization failed: {e}")))?;
        let png: Robj = png.map_or_else(|| Robj::from(()), |b| Raw::from_bytes(&b).into());
        Ok(list!(json = text, png = png))
    });
    done(res)
}

/// `chromatogram` query (the arguments of MCP `openreadout_chromatogram`).
#[extendr]
fn rs_chromatogram(h: Robj, query: &str) -> Robj {
    json(
        parse::<openreadout_quant::api::ChromatogramQuery>("chromatogram query", query).and_then(
            |q| {
                with_ds(&h, |ds, _| {
                    let info = ds.info()?;
                    openreadout_quant::api::chromatogram(ds, &info, &q, &Default::default())
                })
            },
        ),
    )
}

/// `peaks` query (the arguments of MCP `openreadout_peaks`):
/// `{"output": PeaksOutput, "rows": [PeakRow]}`.
#[extendr]
fn rs_peaks(h: Robj, query: &str) -> Robj {
    json(
        parse::<openreadout_quant::api::PeaksQuery>("peaks query", query).and_then(|q| {
            with_ds(&h, |ds, _| {
                let info = ds.info()?;
                let out = openreadout_quant::api::peaks(ds, &info, &q, &Default::default())?;
                // spectra: one row per band, regions apart
                let rows = if out.chromatograms.is_empty() && !out.spectra.is_empty() {
                    serde_json::to_value(out.band_rows())
                } else {
                    serde_json::to_value(out.peak_rows())
                }
                .unwrap_or_default();
                let regions = out.region_rows();
                Ok(serde_json::json!({ "output": out, "rows": rows, "regions": regions }))
            })
        }),
    )
}

/// Per-well statistics of a multi-well plate (`stats --per well --json` data).
#[extendr]
fn rs_well_stats(h: Robj, select: Vec<String>, wells: Vec<String>, per_field: bool) -> Robj {
    json(with_ds(&h, |ds, _| {
        let info = ds.info()?;
        let mut req = openreadout_core::plate::WellStatsRequest::default();
        req.select = select;
        req.wells = wells;
        req.per_field = per_field;
        openreadout_core::plate::well_stats(
            ds,
            &info,
            &req,
            &openreadout_core::parallel::ReadContext::default(),
        )
    }))
}

/// A batch table (`BatchToolArgs` JSON in, `BatchOutput` JSON out, every row).
#[extendr]
fn rs_batch(request: &str) -> Robj {
    json(
        parse::<openreadout_batch::api::BatchToolArgs>("batch request", request)
            .and_then(|a| openreadout_batch::api::run_batch(&registry(), a, false)),
    )
}

/// Group summary of a table file (`SummarizeToolArgs` JSON in, `SummarizeToolOutput` out).
#[extendr]
fn rs_summarize(request: &str) -> Robj {
    json(
        parse::<openreadout_batch::api::SummarizeToolArgs>("summarize request", request)
            .and_then(openreadout_batch::api::run_summarize),
    )
}

/// Files of the same sample (`LinkToolArgs` JSON in, `LinkOutput` out).
#[extendr]
fn rs_link(request: &str) -> Robj {
    json(
        parse::<openreadout_batch::api::LinkToolArgs>("link request", request)
            .and_then(|a| openreadout_batch::api::run_link(&registry(), a)),
    )
}

/// qPCR records and analyses (`openreadout analyze qpcr --json` data).
#[extendr]
fn rs_qpcr(path: &str, request: &str) -> Robj {
    json(
        parse::<export::QpcrArgs>("qpcr request", request).and_then(|a| {
            let ds = openreadout_qpcr::open_qpcr(&registry(), Path::new(path))?;
            openreadout_qpcr::qpcr_report(&ds, &a.request())
        }),
    )
}

/// Export to `to` (`ome-tiff`, `ome-zarr`, `mzml`, `parquet`, `arrow`, `rdml`); `options` is a
/// JSON object of export options. Returns the export report as JSON.
#[extendr]
fn rs_export(h: Robj, to: &str, output: &str, options: &str) -> Robj {
    json(export::export(&h, to, output, options))
}

extendr_module! {
    mod openreadout;
    fn rs_open;
    fn rs_close;
    fn rs_handle_state;
    fn rs_version;
    fn rs_formats;
    fn rs_detect;
    fn rs_info;
    fn rs_info_view;
    fn rs_stats;
    fn rs_check;
    fn rs_ome_xml;
    fn rs_read_planes;
    fn rs_columnar;
    fn rs_spectrum;
    fn rs_scans;
    fn rs_analyze_dataset;
    fn rs_analyze;
    fn rs_chromatogram;
    fn rs_peaks;
    fn rs_well_stats;
    fn rs_batch;
    fn rs_summarize;
    fn rs_link;
    fn rs_qpcr;
    fn rs_export;
}

#[cfg(test)]
mod tests {
    /// The `#[extendr]` macros generate this crate's only unsafe code.
    #[test]
    fn no_unsafe_in_source() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            let text = std::fs::read_to_string(&p).unwrap();
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            let keyword = ["un", "safe"].concat();
            assert!(
                !code
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|w| w == keyword),
                "{} uses the forbidden keyword",
                p.display()
            );
        }
    }

    /// Same readers, same order as the CLI binary.
    #[test]
    fn registry_matches_cli() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let readers = |text: &str| -> Vec<String> {
            text.lines()
                .map(str::trim)
                .filter(|l| l.starts_with(".with(Box::new("))
                .map(str::to_string)
                .collect()
        };
        let cli = std::fs::read_to_string(root.join("../openreadout-cli/src/registry.rs")).unwrap();
        let ours = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert_eq!(readers(&ours), readers(&cli));
    }
}
