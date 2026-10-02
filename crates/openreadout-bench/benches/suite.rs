//! OpenReadout benchmark suite (criterion).
//!
//! ```text
//! cargo bench -p openreadout-bench                 # everything (about 5 minutes)
//! cargo bench -p openreadout-bench -- info/        # one group: info/ decode/ codec/ export/ mzml/ trace/
//! ```
//!
//! Groups:
//! - `info/<family>`: detect + header-only open + `info()` (what `openreadout info` does).
//! - `codec/<codec>`: the pure decoders on synthetic payloads (always available).
//! - `decode/<format>-<codec>`: one full plane through a reader (corpus files).
//! - `export/<target>/<input>`: OME-TIFF / OME-Zarr export including read-back verification.
//! - `mzml/<input>`: open + every spectrum decoded.
//! - `trace/<input>`: one full sweep of every channel in physical units.
//!
//! Corpus inputs come from `$OPENREADOUT_CORPUS_DIR` (default `corpus/files`); an absent file
//! is skipped with a note on stderr. Synthetic inputs are generated into a temporary directory,
//! so the suite always has something to measure.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use openreadout_bench::{
    CodecInputs, SyntheticImage, corpus_file, registry, tiff_options, write_mzml, write_ome_tiff,
    write_spikeglx, zarr_options,
};
use openreadout_core::parallel::ReadContext;
use openreadout_core::reader::{Dataset, PlaneIndex, Registry};

/// Synthetic inputs shared by every group, written once.
struct Fixtures {
    _dir: tempfile::TempDir,
    ome_tiff: PathBuf,
    ome_tiff_deflate: PathBuf,
    ome_tiff_lzw: PathBuf,
    mzml: PathBuf,
    spikeglx: PathBuf,
}

fn fixtures() -> &'static Fixtures {
    static F: std::sync::OnceLock<Fixtures> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut img = SyntheticImage::new(1024, 1024, 2, 4, 1);
        let ome_tiff = dir.path().join("synthetic.ome.tiff");
        write_ome_tiff(&mut img, &ome_tiff, openreadout_ometiff::Codec::None)
            .expect("synthetic OME-TIFF");
        let ome_tiff_deflate = dir.path().join("synthetic-deflate.ome.tiff");
        write_ome_tiff(
            &mut img,
            &ome_tiff_deflate,
            openreadout_ometiff::Codec::Deflate,
        )
        .expect("synthetic OME-TIFF (deflate)");
        let ome_tiff_lzw = dir.path().join("synthetic-lzw.ome.tiff");
        write_ome_tiff(&mut img, &ome_tiff_lzw, openreadout_ometiff::Codec::Lzw)
            .expect("synthetic OME-TIFF (LZW)");
        let mzml = dir.path().join("synthetic.mzML");
        write_mzml(200, 2000, &mzml).expect("synthetic mzML");
        let spikeglx = write_spikeglx(dir.path(), 16, 300_000).expect("synthetic SpikeGLX");
        Fixtures {
            _dir: dir,
            ome_tiff,
            ome_tiff_deflate,
            ome_tiff_lzw,
            mzml,
            spikeglx,
        }
    })
}

fn skip(what: &str, file: &str) {
    eprintln!("skipping {what}: corpus file {file} not found (set OPENREADOUT_CORPUS_DIR)");
}

/// Corpus inputs for `info`, one or two per format family (all smoke tier).
const INFO_FILES: &[(&str, &str)] = &[
    ("czi", "aics-s-3-t-1-c-3-z-5.czi"),
    ("nd2", "ome-aryeh-MeOh-high-fluo-003.nd2"),
    ("lif", "zenodo14976703-Convalaria-LambdaScan.lif"),
    ("ome-tiff", "aics-s-3-t-1-c-3-z-5.ome.tiff"),
    ("svs", "openslide-aperio-CMU-1-Small-Region.svs"),
    ("zvi", "figshare-zvi/figshare26880337-Figure_1H.zvi"),
    ("oir", "zenodo13680725-map-a01.oir"),
    ("ims", "ome-imaris/Convallaria_3C_1T_confocal.ims"),
    ("ome-zarr", "zenodo13982701-220605_151046_mip.zarr.zip"),
    ("fcs", "fcsparser-facs-diva.fcs"),
    ("abf", "pyabf-2020-07-29-0062.abf"),
    ("thermo-raw", "mtbls20-caffeine-pos.raw"),
    ("mzml", "mzdata-small.mzML"),
];

fn open_info(reg: &Registry, path: &Path) {
    let (_, ds) = reg.open(path).expect("open");
    black_box(ds.info().expect("info"));
}

fn bench_info(c: &mut Criterion) {
    let reg = registry();
    let fx = fixtures();
    let mut g = c.benchmark_group("info");
    g.measurement_time(Duration::from_secs(3));
    for (label, path) in [
        ("synthetic-ome-tiff", &fx.ome_tiff),
        ("synthetic-mzml", &fx.mzml),
        ("synthetic-spikeglx", &fx.spikeglx),
    ] {
        g.bench_function(label, |b| b.iter(|| open_info(&reg, path)));
    }
    for &(family, file) in INFO_FILES {
        let Some(path) = corpus_file(file) else {
            skip(&format!("info/{family}"), file);
            continue;
        };
        g.bench_function(family, |b| b.iter(|| open_info(&reg, &path)));
    }
    g.finish();
}

fn bench_codecs(c: &mut Criterion) {
    use openreadout_codecs as codecs;
    let inp = CodecInputs::new(1024, 1024);
    let n = inp.raw.len();
    let mut g = c.benchmark_group("codec");
    g.measurement_time(Duration::from_secs(3));
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function("raw-copy", |b| b.iter(|| black_box(inp.raw.clone())));
    g.bench_function("zstd", |b| {
        b.iter(|| codecs::zstd_decode(black_box(&inp.zstd), n).unwrap());
    });
    g.bench_function("zstd1-hilo", |b| {
        b.iter(|| codecs::zstd1_decode(black_box(&inp.zstd1_hilo), n, 2).unwrap());
    });
    g.bench_function("hilo-unshuffle", |b| {
        b.iter(|| codecs::unshuffle_hilo(black_box(&inp.raw)));
    });
    g.bench_function("lzw", |b| {
        b.iter(|| codecs::lzw_decode(black_box(&inp.lzw), n).unwrap());
    });
    g.bench_function("deflate", |b| {
        b.iter(|| codecs::zlib_decode(black_box(&inp.zlib), n).unwrap());
    });
    // JPEG and JPEG 2000 throughput is in decoded bytes too (8-bit RGB and 16-bit gray).
    g.throughput(Throughput::Bytes(
        u64::from(inp.width) * u64::from(inp.height) * 3,
    ));
    g.bench_function("jpeg", |b| {
        b.iter(|| codecs::jpeg_decode(black_box(&inp.jpeg)).unwrap());
    });
    g.throughput(Throughput::Bytes(n as u64));
    g.sample_size(10);
    g.bench_function("jpeg2000", |b| {
        b.iter(|| codecs::jpeg2000_decode(black_box(&inp.jpeg2000)).unwrap());
    });
    g.finish();
}

/// Corpus inputs for plane decode: (label, file, image). JPEG XR has no pure-Rust encoder,
/// so it is measured only here.
const DECODE_FILES: &[(&str, &str, u32)] = &[
    ("czi-raw", "aics-s-3-t-1-c-3-z-5.czi", 0),
    ("czi-zstd0", "openslide-zeiss-5-slidepreview-zstd0.czi", 0),
    (
        "czi-zstd1-hilo",
        "openslide-zeiss-5-slidepreview-zstd1-hilo.czi",
        0,
    ),
    ("czi-jpegxr", "openslide-zeiss-5-slidepreview-jxr.czi", 0),
    ("czi-jpeg", "synthetic-gray8-jpeg.czi", 0),
    ("nd2-raw", "ome-aryeh-MeOh-high-fluo-003.nd2", 0),
    ("lif-raw", "zenodo14976703-Convalaria-LambdaScan.lif", 0),
    ("tiff-lzw", "aics-4c_3Z_pyramid.tiff", 0),
    (
        "tiff-deflate",
        "aics-s_1_t_1_c_1_z_1_ome_tiff_tiles.ome.tif",
        0,
    ),
    ("svs-jpeg", "openslide-aperio-CMU-1-Small-Region.svs", 0),
    (
        "vsi-jpeg2000",
        "zenodo6094961-vsi-multifile/data/vsi-ets-test-jpg2k.vsi",
        0,
    ),
];

fn bench_plane(
    g: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    reg: &Registry,
    label: &str,
    path: &Path,
    image: u32,
) {
    let (_, mut ds) = match reg.open(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping decode/{label}: {e}");
            return;
        }
    };
    let idx = PlaneIndex::default();
    let bytes = match ds.read_plane(image, idx) {
        Ok(p) => p.data.len() as u64,
        Err(e) => {
            eprintln!("skipping decode/{label}: {e}");
            return;
        }
    };
    g.throughput(Throughput::Bytes(bytes));
    g.bench_function(label, |b| {
        b.iter(|| black_box(ds.read_plane(image, idx).unwrap()));
    });
}

fn bench_decode(c: &mut Criterion) {
    let reg = registry();
    let fx = fixtures();
    let mut g = c.benchmark_group("decode");
    g.measurement_time(Duration::from_secs(3));
    g.sample_size(20);
    bench_plane(&mut g, &reg, "synthetic-tiff-raw", &fx.ome_tiff, 0);
    bench_plane(
        &mut g,
        &reg,
        "synthetic-tiff-deflate",
        &fx.ome_tiff_deflate,
        0,
    );
    bench_plane(&mut g, &reg, "synthetic-tiff-lzw", &fx.ome_tiff_lzw, 0);
    for &(label, file, image) in DECODE_FILES {
        match corpus_file(file) {
            Some(path) => bench_plane(&mut g, &reg, label, &path, image),
            None => skip(&format!("decode/{label}"), file),
        }
    }
    g.finish();
}

fn export_once(ds: &mut dyn Dataset, input: &Path, out: &Path, zarr: bool, ctx: &ReadContext<'_>) {
    if zarr {
        let r = openreadout_omezarr::export_ome_zarr_with(
            ds,
            input,
            out,
            &zarr_options(openreadout_ometiff::Codec::Deflate, None),
            ctx,
        );
        black_box(r.expect("OME-Zarr export"));
    } else {
        let r = openreadout_ometiff::export_ome_tiff_with(
            ds,
            input,
            out,
            &tiff_options(openreadout_ometiff::Codec::None),
            ctx,
        );
        black_box(r.expect("OME-TIFF export"));
    }
}

fn bench_export(c: &mut Criterion) {
    let reg = registry();
    let out_dir = tempfile::tempdir().expect("temporary directory");
    let mut g = c.benchmark_group("export");
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(8));
    // Single-threaded: the rayon pool is not consulted without an opener.
    let seq = ReadContext::default();
    let synth = SyntheticImage::new(1024, 1024, 2, 4, 2);
    g.throughput(Throughput::Bytes(synth.total_bytes()));
    for (target, zarr, ext) in [
        ("ome-tiff", false, "ome.tiff"),
        ("ome-zarr", true, "ome.zarr"),
    ] {
        let out = out_dir.path().join(format!("synthetic.{ext}"));
        let mut ds = synth.clone();
        g.bench_function(format!("{target}/synthetic"), |b| {
            b.iter(|| export_once(&mut ds, Path::new("synthetic"), &out, zarr, &seq));
        });
    }
    let file = "aics-s-3-t-1-c-3-z-5.czi";
    if let Some(path) = corpus_file(file) {
        let (_, mut ds) = reg.open(&path).expect("open");
        let info = ds.info().expect("info");
        let bytes: u64 = info
            .images
            .iter()
            .map(|i| {
                u64::from(i.size_x)
                    * u64::from(i.size_y)
                    * u64::from(i.samples_per_pixel.max(1))
                    * i.pixel_type.bytes_per_sample() as u64
                    * i.plane_count
            })
            .sum();
        g.throughput(Throughput::Bytes(bytes));
        for (target, zarr, ext) in [
            ("ome-tiff", false, "ome.tiff"),
            ("ome-zarr", true, "ome.zarr"),
        ] {
            let out = out_dir.path().join(format!("czi.{ext}"));
            g.bench_function(format!("{target}/czi"), |b| {
                b.iter(|| export_once(ds.as_mut(), &path, &out, zarr, &seq));
            });
        }
    } else {
        skip("export/*/czi", file);
    }
    g.finish();
}

fn read_all_spectra(reg: &Registry, path: &Path) -> u64 {
    let (_, mut ds) = reg.open(path).expect("open");
    let info = ds.info().expect("info");
    let mut points = 0u64;
    for run in &info.spectra {
        for i in 0..run.scan_count {
            points += ds.read_spectrum(run.index, i).expect("spectrum").mz.len() as u64;
        }
    }
    points
}

fn bench_mzml(c: &mut Criterion) {
    let reg = registry();
    let fx = fixtures();
    let mut g = c.benchmark_group("mzml");
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(5));
    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    g.throughput(Throughput::Bytes(size(&fx.mzml)));
    g.bench_function("synthetic", |b| {
        b.iter(|| black_box(read_all_spectra(&reg, &fx.mzml)));
    });
    for file in ["mzdata-small.mzML", "synthetic-mzml-numpress.mzML"] {
        let Some(path) = corpus_file(file) else {
            skip(&format!("mzml/{file}"), file);
            continue;
        };
        g.throughput(Throughput::Bytes(size(&path)));
        g.bench_function(file, |b| {
            b.iter(|| black_box(read_all_spectra(&reg, &path)));
        });
    }
    g.finish();
}

fn read_sweep(reg: &Registry, path: &Path) -> usize {
    let (_, mut ds) = reg.open(path).expect("open");
    let info = ds.info().expect("info");
    let t = &info.traces[0];
    let n = openreadout_core::trace::sweep_samples(t, 0);
    let tr = ds.read_trace(t.index, 0, 0, n).expect("trace");
    tr.channels.iter().map(Vec::len).sum()
}

fn bench_trace(c: &mut Criterion) {
    let reg = registry();
    let fx = fixtures();
    let mut g = c.benchmark_group("trace");
    g.measurement_time(Duration::from_secs(3));
    g.sample_size(20);
    g.throughput(Throughput::Elements(16 * 300_000));
    g.bench_function("synthetic-spikeglx", |b| {
        b.iter(|| black_box(read_sweep(&reg, &fx.spikeglx)));
    });
    for file in ["pyabf-2020-07-29-0062.abf", "pyabf-18425108.abf"] {
        let Some(path) = corpus_file(file) else {
            skip(&format!("trace/{file}"), file);
            continue;
        };
        let samples = read_sweep(&reg, &path) as u64;
        g.throughput(Throughput::Elements(samples));
        g.bench_function(file, |b| b.iter(|| black_box(read_sweep(&reg, &path))));
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_info,
    bench_codecs,
    bench_decode,
    bench_export,
    bench_mzml,
    bench_trace
);
criterion_main!(benches);
