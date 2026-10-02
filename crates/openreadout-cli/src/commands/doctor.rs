//! `self doctor`: report the build (version, target, features, formats, MCP) and run a self-test on
//! tiny synthetic files generated in memory: a 16×8 two-page uint16 TIFF and an FCS 3.1 file
//! with 10 events × 3 float parameters (also written by `--write-fixtures`), then an mzML run,
//! an RDML file, a long-form plate CSV, a Gating-ML gate and a JCAMP-DX spectrum for the
//! analysis commands (`doctor_analysis`). No corpus file is embedded or needed.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::reader::PlaneIndex;
use openreadout_core::stats::{HistogramScale, StatsRequest, compute_stats};
use openreadout_core::{Error, Registry, Result};
use openreadout_ops::compare::{CompareRequest, compare};
use schemars::JsonSchema;
use serde::Serialize;

use crate::output::{emit, fail};
use crate::ui;

/// Arguments of `doctor`.
#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Write the two self-test files (`doctor.tif`, `doctor.fcs`) into DIR and exit, e.g. to
    /// attach to a bug report or to try commands without data of your own.
    #[arg(long, value_name = "DIR")]
    pub write_fixtures: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

/// One self-test step.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Output of `doctor`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DoctorReport {
    /// True when every check passed (exit 0; otherwise exit 1).
    pub ok: bool,
    pub version: String,
    /// Target triple the binary was built for.
    pub target: String,
    /// `release` or `debug`.
    pub profile: String,
    /// Cargo features compiled in.
    pub features: Vec<String>,
    /// Whether `openreadout mcp` can serve (the `mcp` feature).
    pub mcp: bool,
    /// The MCP tools the server offers (empty without the `mcp` feature).
    pub mcp_tools: Vec<String>,
    /// Worker threads for plane decoding (`--threads`).
    pub threads: usize,
    /// Registered format readers, in detection order.
    pub formats: Vec<String>,
    pub stdout_is_terminal: bool,
    pub stderr_is_terminal: bool,
    /// Environment variables that change behaviour, when set.
    pub environment: Vec<String>,
    pub checks: Vec<DoctorCheck>,
}

const W: u32 = 16;
const H: u32 = 8;

/// Sample (x, y) of TIFF page `p`.
fn tiff_value(p: u32, x: u32, y: u32) -> u16 {
    (p * 1000 + y * W + x) as u16
}

/// Little-endian bytes of TIFF page `p`.
pub fn tiff_page(p: u32) -> Vec<u8> {
    (0..H)
        .flat_map(|y| (0..W).map(move |x| tiff_value(p, x, y)))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A classic little-endian TIFF with two uncompressed 16×8 uint16 grayscale pages.
pub fn tiff_fixture() -> Vec<u8> {
    const ENTRIES: u16 = 10;
    let ifd_len = 2 + 12 * u32::from(ENTRIES) + 4;
    let page_len = W * H * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    for p in 0..2u32 {
        let ifd = 8 + p * (ifd_len + page_len);
        let data = ifd + ifd_len;
        let next = if p == 0 { data + page_len } else { 0 };
        out.extend_from_slice(&ENTRIES.to_le_bytes());
        let mut entry = |tag: u16, typ: u16, value: u32| {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            if typ == 3 {
                out.extend_from_slice(&(value as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            } else {
                out.extend_from_slice(&value.to_le_bytes());
            }
        };
        entry(256, 3, W); // ImageWidth
        entry(257, 3, H); // ImageLength
        entry(258, 3, 16); // BitsPerSample
        entry(259, 3, 1); // Compression: none
        entry(262, 3, 1); // Photometric: min-is-black
        entry(273, 4, data); // StripOffsets
        entry(277, 3, 1); // SamplesPerPixel
        entry(278, 3, H); // RowsPerStrip
        entry(279, 4, page_len); // StripByteCounts
        entry(339, 3, 1); // SampleFormat: unsigned
        out.extend_from_slice(&next.to_le_bytes());
        out.extend_from_slice(&tiff_page(p));
    }
    out
}

const EVENTS: usize = 10;
const PARAMS: [&str; 3] = ["FSC-A", "SSC-A", "Time"];

fn fcs_value(event: usize, param: usize) -> f32 {
    (event * 10 + param) as f32 + 0.5
}

/// An FCS 3.1 file: 10 events × 3 float32 parameters, little-endian, list mode.
pub fn fcs_fixture() -> Vec<u8> {
    let data: Vec<u8> = (0..EVENTS)
        .flat_map(|e| (0..PARAMS.len()).map(move |p| fcs_value(e, p)))
        .flat_map(f32::to_le_bytes)
        .collect();
    let text_start = 58usize;
    // Offsets are zero-padded to a fixed width so the TEXT length does not depend on them.
    let build_text = |data_start: usize, data_end: usize| {
        let mut kv: Vec<(String, String)> = vec![
            ("$BEGINANALYSIS".into(), "0".into()),
            ("$ENDANALYSIS".into(), "0".into()),
            ("$BEGINSTEXT".into(), "0".into()),
            ("$ENDSTEXT".into(), "0".into()),
            ("$BEGINDATA".into(), format!("{data_start:08}")),
            ("$ENDDATA".into(), format!("{data_end:08}")),
            ("$BYTEORD".into(), "1,2,3,4".into()),
            ("$DATATYPE".into(), "F".into()),
            ("$MODE".into(), "L".into()),
            ("$NEXTDATA".into(), "0".into()),
            ("$PAR".into(), PARAMS.len().to_string()),
            ("$TOT".into(), EVENTS.to_string()),
            ("$CYT".into(), "openreadout doctor".into()),
        ];
        for (i, n) in PARAMS.iter().enumerate() {
            let k = i + 1;
            kv.push((format!("$P{k}N"), (*n).to_string()));
            kv.push((format!("$P{k}B"), "32".into()));
            kv.push((format!("$P{k}E"), "0,0".into()));
            kv.push((format!("$P{k}R"), "1024".into()));
        }
        let mut t = String::from("|");
        for (k, v) in kv {
            t.push_str(&k);
            t.push('|');
            t.push_str(&v);
            t.push('|');
        }
        t.into_bytes()
    };
    let len = build_text(0, 0).len();
    let data_start = text_start + len;
    let data_end = data_start + data.len() - 1;
    let text = build_text(data_start, data_end);
    let mut out = format!(
        "FCS3.1    {:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
        text_start,
        text_start + text.len() - 1,
        data_start,
        data_end,
        0,
        0
    )
    .into_bytes();
    out.extend_from_slice(&text);
    out.extend_from_slice(&data);
    out
}

/// Write both fixtures into `dir`.
pub fn write_fixtures(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let t = dir.join("doctor.tif");
    let f = dir.join("doctor.fcs");
    std::fs::write(&t, tiff_fixture()).map_err(|e| Error::io(&t, e))?;
    std::fs::write(&f, fcs_fixture()).map_err(|e| Error::io(&f, e))?;
    Ok((t, f))
}

pub(crate) fn check(name: &str, f: impl FnOnce() -> Result<String>) -> DoctorCheck {
    match f() {
        Ok(detail) => DoctorCheck {
            name: name.into(),
            ok: true,
            detail,
        },
        Err(e) => DoctorCheck {
            name: name.into(),
            ok: false,
            detail: e.to_string(),
        },
    }
}

pub(crate) fn expect(cond: bool, what: impl Into<String>) -> Result<()> {
    if cond {
        Ok(())
    } else {
        Err(Error::Other(what.into()))
    }
}

fn self_test(reg: &Registry, dir: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let (tif, fcs) = match write_fixtures(dir) {
        Ok(v) => v,
        Err(e) => {
            checks.push(DoctorCheck {
                name: "write fixtures".into(),
                ok: false,
                detail: e.to_string(),
            });
            return checks;
        }
    };
    checks.push(check("detect tiff", || {
        let (_, d) = reg.detect(&tif)?;
        expect(d.format_id == "tiff", format!("detected {}", d.format_id))?;
        Ok(format!("{} ({:?})", d.format_id, d.confidence))
    }));
    checks.push(check("tiff info and planes", || {
        let (_, mut ds) = reg.open(&tif)?;
        let info = ds.info()?;
        let im = info
            .images
            .first()
            .ok_or_else(|| Error::Other("no image".into()))?;
        expect(
            im.size_x == W && im.size_y == H && im.size_c * im.size_z * im.size_t == 2,
            format!(
                "geometry {}x{} with {} planes",
                im.size_x,
                im.size_y,
                im.size_c * im.size_z * im.size_t
            ),
        )?;
        let mut n = 0u32;
        for t in 0..im.size_t {
            for z in 0..im.size_z {
                for c in 0..im.size_c {
                    let plane = ds.read_plane(0, PlaneIndex { c, z, t })?;
                    expect(
                        plane.data == tiff_page(n),
                        format!("page {n} samples differ"),
                    )?;
                    n += 1;
                }
            }
        }
        Ok(format!(
            "{}x{} uint16, {n} planes bit-exact",
            im.size_x, im.size_y
        ))
    }));
    checks.push(check("tiff check", || {
        let (_, mut ds) = reg.open(&tif)?;
        let r = ds.check()?;
        expect(r.ok, format!("{} findings", r.findings.len()))?;
        Ok(format!("{} checks performed", r.checks_performed.len()))
    }));
    checks.push(check("tiff stats", || {
        let (_, mut ds) = reg.open(&tif)?;
        let info = ds.info()?;
        let s = compute_stats(
            ds.as_mut(),
            &info,
            &{
                let mut stats_request = StatsRequest::default();
                stats_request.bins = 8;
                stats_request.scale = HistogramScale::Linear;
                stats_request
            },
            &ReadContext::default(),
        )?;
        let im = &s.images[0].stats;
        let want_max = f64::from(tiff_value(1, W - 1, H - 1));
        expect(
            im.min == Some(0.0) && im.max == Some(want_max) && im.count == u64::from(2 * W * H),
            format!("min {:?} max {:?} count {}", im.min, im.max, im.count),
        )?;
        Ok(format!("min 0, max {want_max}, {} samples", im.count))
    }));
    checks.push(check("ome-tiff export round trip", || {
        let out = dir.join("doctor.ome.tiff");
        let (_, mut ds) = reg.open(&tif)?;
        let r = openreadout_ometiff::export_ome_tiff(ds.as_mut(), &tif, &out, &{
            let mut export_options = openreadout_ometiff::ExportOptions::default();
            export_options.image = None;
            export_options.select = Vec::new();
            export_options.codec = openreadout_ometiff::Codec::Deflate;
            export_options.overwrite = true;
            export_options.embed_vendor = false;
            export_options
        })?;
        expect(r.verified, "read-back not verified")?;
        let (_, mut a) = reg.open(&tif)?;
        let (_, mut b) = reg.open(&out)?;
        let (ia, ib) = (a.info()?, b.info()?);
        let c = compare(a.as_mut(), &ia, b.as_mut(), &ib, &{
            let mut compare_request = CompareRequest::default();
            compare_request.no_metadata = true;
            compare_request
        })?;
        expect(
            c.planes.mismatched == 0 && c.planes.planes == 2,
            format!(
                "{} of {} planes differ",
                c.planes.mismatched, c.planes.planes
            ),
        )?;
        Ok(format!(
            "{} planes written, {} bytes, re-read identical",
            r.planes_written, r.bytes_written
        ))
    }));
    checks.push(check("fcs events", || {
        let (_, d) = reg.detect(&fcs)?;
        expect(d.format_id == "fcs", format!("detected {}", d.format_id))?;
        let (_, mut ds) = reg.open(&fcs)?;
        let info = ds.info()?;
        let t = info
            .tables
            .first()
            .ok_or_else(|| Error::Other("no table".into()))?;
        expect(
            t.row_count == EVENTS as u64 && t.columns.len() == PARAMS.len(),
            format!("{} rows x {} columns", t.row_count, t.columns.len()),
        )?;
        let tab = ds.read_table(0, 0, EVENTS as u64)?;
        for (p, col) in tab.columns.iter().enumerate() {
            for (e, v) in col.iter().enumerate() {
                expect(
                    v.to_bits() == f64::from(fcs_value(e, p)).to_bits(),
                    format!("event {e} parameter {p} is {v}"),
                )?;
            }
        }
        let r = ds.check()?;
        expect(r.ok, format!("check: {} findings", r.findings.len()))?;
        Ok(format!(
            "{} events x {} parameters read exactly; check ok",
            t.row_count,
            t.columns.len()
        ))
    }));
    checks.extend(super::doctor_analysis::checks(reg, dir, &tif, &fcs));
    checks
}

#[cfg(feature = "mcp")]
fn mcp_tools() -> Vec<String> {
    openreadout_mcp::InstrumentServer::new(crate::registry::registry).tool_names()
}

#[cfg(not(feature = "mcp"))]
fn mcp_tools() -> Vec<String> {
    Vec::new()
}

fn report(reg: &Registry) -> DoctorReport {
    let dir = std::env::temp_dir().join(format!("openreadout-doctor-{}", std::process::id()));
    let checks = self_test(reg, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    let mut features = Vec::new();
    if cfg!(feature = "mcp") {
        features.push("mcp".to_string());
    }
    let environment = [
        "NO_COLOR",
        "CLICOLOR_FORCE",
        "RUST_BACKTRACE",
        "RAYON_NUM_THREADS",
        "OPENREADOUT_KEEP_PARTIAL",
        "OPENREADOUT_STDIN_MAX_BYTES",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| format!("{k}={v}")))
    .collect();
    DoctorReport {
        ok: checks.iter().all(|c| c.ok),
        version: env!("CARGO_PKG_VERSION").into(),
        target: env!("OPENREADOUT_TARGET").into(),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
        mcp: cfg!(feature = "mcp"),
        mcp_tools: mcp_tools(),
        features,
        threads: rayon::current_num_threads(),
        formats: reg.descriptors().into_iter().map(|d| d.id).collect(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
        stderr_is_terminal: std::io::stderr().is_terminal(),
        environment,
        checks,
    }
}

pub fn run(reg: &Registry, a: &DoctorArgs) -> i32 {
    if let Some(dir) = &a.write_fixtures {
        return match write_fixtures(dir) {
            Ok((t, f)) => {
                if a.json {
                    emit(
                        true,
                        &serde_json::json!({"files": [t.display().to_string(), f.display().to_string()]}),
                        |_| String::new(),
                    )
                } else {
                    if !ui::quiet() {
                        println!("{}\n{}", t.display(), f.display());
                    }
                    0
                }
            }
            Err(e) => fail(a.json, &e),
        };
    }
    let r = report(reg);
    let code = i32::from(!r.ok);
    emit(a.json, &r, render);
    code
}

fn render(r: &DoctorReport) -> String {
    let mut s = format!(
        "openreadout {} ({}, {})\n  features: {}\n  mcp server: {}\n  threads: {}\n  formats ({}): {}\n",
        r.version,
        r.target,
        r.profile,
        if r.features.is_empty() {
            "none".to_string()
        } else {
            r.features.join(", ")
        },
        if r.mcp {
            format!("available ({} tools)", r.mcp_tools.len())
        } else {
            "not compiled in".to_string()
        },
        r.threads,
        r.formats.len(),
        r.formats.join(", ")
    );
    if !r.environment.is_empty() {
        s.push_str(&format!("  environment: {}\n", r.environment.join(" ")));
    }
    s.push_str("self-test:\n");
    for c in &r.checks {
        s.push_str(&format!(
            "  {} {:<28} {}\n",
            if c.ok {
                ui::paint(ui::OK, "ok  ")
            } else {
                ui::paint(ui::ERR, "FAIL")
            },
            c.name,
            c.detail
        ));
    }
    s.push_str(&if r.ok {
        ui::paint(ui::OK, "all checks passed")
    } else {
        ui::paint(ui::ERR, "some checks failed; please report this")
    });
    s
}
