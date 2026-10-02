//! Previews of real files: for up to three inputs per format (≤ 300 MB each), the default
//! preview at 256 px must render, fit the size, encode to a PNG of the reported dimensions, show
//! more than one colour, and (for files ≤ 20 MB) be byte-identical when rendered twice.
//! Readers that cannot provide a drawable view answer "unsupported" (exit 6), which is counted,
//! not failed.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test preview -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_core::{Error, Registry};
use openreadout_preview::{Encoding, PreviewRequest, finish, render};
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    filename: String,
    #[serde(default)]
    role: String,
}

const PER_FORMAT: usize = 3;
const MAX_BYTES: u64 = 300 << 20;
const DETERMINISM_BYTES: u64 = 20 << 20;
const SIZE: u32 = 256;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_fcs::FcsReader))
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
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_mzml::MzmlbReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
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
        .with(Box::new(openreadout_hdf5::Hdf5Reader))
        .with(Box::new(openreadout_plate::PlateReader))
}

fn size_of(p: &Path) -> u64 {
    if p.is_dir() {
        std::fs::read_dir(p).map_or(0, |rd| rd.flatten().map(|e| size_of(&e.path())).sum())
    } else {
        std::fs::metadata(p).map_or(0, |m| m.len())
    }
}

fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    (b.len() > 24 && &b[..8] == b"\x89PNG\r\n\x1a\n").then(|| {
        (
            u32::from_be_bytes([b[16], b[17], b[18], b[19]]),
            u32::from_be_bytes([b[20], b[21], b[22], b[23]]),
        )
    })
}

/// Render the default preview; `Ok(None)` when the reader has nothing drawable (unsupported).
/// A rendered preview: (kind, PNG bytes, width, height, source bytes read).
type Rendered = (String, Vec<u8>, u32, u32, usize);

fn preview(reg: &Registry, path: &Path) -> Result<Option<Rendered>, String> {
    let (_, mut ds) = match reg.open(path) {
        Ok(x) => x,
        Err(Error::UnknownFormat { .. } | Error::Unsupported { .. }) => return Ok(None),
        Err(e) => return Err(format!("open: {e}")),
    };
    let info = match ds.info() {
        Ok(i) => i,
        Err(Error::Unsupported { .. }) => return Ok(None),
        Err(e) => return Err(format!("info: {e}")),
    };
    let req = {
        let mut preview_request = PreviewRequest::default();
        preview_request.max_size = SIZE;
        preview_request
    };
    let r = match render(ds.as_mut(), &info, &req) {
        Ok(r) => r,
        Err(Error::Unsupported { .. }) => return Ok(None),
        Err(e) => return Err(format!("render: {e}")),
    };
    // Colours of the data only: the rulers' frame around image previews is not variance.
    let colours: BTreeSet<&[u8]> = match &r.output.image {
        Some(im) => {
            let pa = im.plot_area;
            let w = r.canvas.width as usize;
            (pa.y as usize..(pa.y + pa.height) as usize)
                .flat_map(|y| {
                    let row = y * w + pa.x as usize;
                    r.canvas.rgb[row * 3..(row + pa.width as usize) * 3].chunks(3)
                })
                .collect()
        }
        None => r.canvas.rgb.chunks(3).collect(),
    };
    let (out, bytes) = finish(&r, Encoding::Png, 90).map_err(|e| format!("encode: {e}"))?;
    Ok(Some((
        out.kind,
        bytes,
        out.width,
        out.height,
        colours.len(),
    )))
}

/// Axioscan RAC scans (12-bit in uint16): their pyramid tiles hold 65535 past the scanned tiles
/// and 65535 blended with data along the tile edges, while level 0 composites uncovered canvas
/// as 0. The default preview reads a pyramid level; its contrast must come from the 12-bit data
/// and, in the Intestine scan, the fill must be drawn as background (it was drawn white, with
/// the display range 0–65535 leaving the tissue near black).
#[test]
fn slide_scan_fill_does_not_set_the_preview() {
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from);
    let reg = registry();
    // (file, fill drawn as background)
    let cases = [
        ("zenodo10577621-Intestine-3color-RAC.czi", true),
        ("zenodo10577621-Kidney-RAC-3color.czi", false),
    ];
    let mut ran = 0;
    for (name, fill) in cases {
        let path = files_dir.join(name);
        if !path.exists() {
            println!("skip {name}: not fetched");
            continue;
        }
        ran += 1;
        let (_, mut ds) = reg.open(&path).unwrap();
        let info = ds.info().unwrap();
        // Each channel alone, in grey (a composite adds the bright TL Brightfield channel).
        for c in 0..info.images[0].size_c {
            let mut req = PreviewRequest::default();
            req.axes = false;
            req.select = vec![format!("c={c}")];
            let r = render(ds.as_mut(), &info, &req).unwrap();
            let im = r.output.image.as_ref().unwrap();
            assert!(
                im.level > 0,
                "{name}: the default preview reads a pyramid level"
            );
            let ch = &im.channels[0];
            assert!(
                ch.display_max <= 4095.0,
                "{name} c={c}: white point {} beyond the 12-bit data",
                ch.display_max
            );
            // Grey pixels: the first of the three equal samples.
            let grey = || r.canvas.rgb.chunks(3).map(|p| p[0]);
            let px = grey().count();
            let white = grey().filter(|&v| v == 255).count();
            let black = grey().filter(|&v| v == 0).count();
            println!(
                "{name} c={c}: level {} {}x{}, display {:.0}..{:.0}, white {white}, black {black} of {px}; {:?}",
                im.level,
                r.canvas.width,
                r.canvas.height,
                ch.display_min,
                ch.display_max,
                r.output.notes
            );
            // Unscreened, the fill drew 12.6% of the Intestine preview white.
            assert!(
                white * 100 < px,
                "{name} c={c}: {white} of {px} pixels white"
            );
            let notes = r.output.notes.join("\n");
            if fill {
                assert!(notes.contains("drawn as background"), "{name}: {notes}");
                // Uncovered canvas is 37% of level 0; black at this level too.
                assert!(black * 4 > px, "{name} c={c}: {black} of {px} pixels black");
            } else {
                assert!(
                    notes.contains("left out of the contrast"),
                    "{name}: {notes}"
                );
            }
        }
    }
    println!("checked {ran} slide scans");
}

#[test]
fn corpus_previews_render() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let mut per_format: BTreeMap<String, usize> = BTreeMap::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut unsupported = Vec::new();
    let mut failures = Vec::new();
    let mut rendered = 0usize;
    for e in manifest
        .file
        .iter()
        .filter(|e| e.role.is_empty() || e.role == "input")
    {
        if only.as_ref().is_some_and(|o| !e.id.contains(o.as_str())) {
            continue;
        }
        let path = files_dir.join(&e.filename);
        if !path.exists() {
            continue;
        }
        let bytes_on_disk = size_of(&path);
        if bytes_on_disk > MAX_BYTES {
            continue;
        }
        let Ok((_, det)) = reg.detect(&path) else {
            continue;
        };
        let n = per_format.entry(det.format_id.to_string()).or_default();
        if *n >= PER_FORMAT {
            continue;
        }
        *n += 1;
        match preview(&reg, &path) {
            Err(msg) => failures.push(format!("{}: {msg}", e.id)),
            Ok(None) => unsupported.push(e.id.clone()),
            Ok(Some((kind, png, w, h, colours))) => {
                rendered += 1;
                *kinds.entry(kind.clone()).or_default() += 1;
                let mut bad = Vec::new();
                if w == 0 || h == 0 || w.max(h) > SIZE {
                    bad.push(format!("size {w}x{h} (max {SIZE})"));
                }
                if png_size(&png) != Some((w, h)) {
                    bad.push("PNG header disagrees with the report".into());
                }
                if colours < 2 {
                    bad.push("a single colour (no variance)".into());
                }
                if bytes_on_disk <= DETERMINISM_BYTES {
                    match preview(&reg, &path) {
                        Ok(Some((_, again, ..))) if again == png => {}
                        _ => bad.push("second render differs".into()),
                    }
                }
                println!(
                    "{:<60} {kind:<8} {w}x{h} {colours} colours {}",
                    e.id,
                    if bad.is_empty() { "ok" } else { "FAIL" }
                );
                if !bad.is_empty() {
                    failures.push(format!("{}: {}", e.id, bad.join("; ")));
                }
            }
        }
    }
    println!(
        "rendered {rendered} previews over {} formats {kinds:?}; unsupported (no drawable view): {}",
        per_format.len(),
        unsupported.join(", ")
    );
    assert!(
        failures.is_empty(),
        "preview failures:\n{}",
        failures.join("\n")
    );
}
