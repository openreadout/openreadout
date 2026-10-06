//! `info` reads headers only (AGENTS.md): how many bytes it reads must not grow with the pixel,
//! array or sample data. Each test writes a synthetic file, opens it through a byte source that
//! counts every byte read, runs `info`, and checks the count against a small budget.
//!
//! The budgets are a few times what the readers read today, so a reader that starts touching
//! the data (decoding a plane, scanning arrays, reading a recording through) fails here.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use openreadout_bench::{SyntheticImage, registry, write_mzml, write_ome_tiff, write_spikeglx};
use openreadout_core::source::{ByteSource, DirEntry, DirSource, EntryMeta, Fs, Input, LocalFs};

/// A file whose reads are added to a shared counter.
#[derive(Debug)]
struct Counted {
    inner: Arc<dyn ByteSource>,
    read: Arc<AtomicU64>,
}

impl ByteSource for Counted {
    fn size(&self) -> io::Result<u64> {
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read_at(offset, buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn name(&self) -> String {
        self.inner.name()
    }
    // `local_path` stays `None`, so readers cannot go around the counter.
}

/// The local file system, with every byte read counted.
#[derive(Debug, Default)]
struct CountingFs {
    read: Arc<AtomicU64>,
}

impl DirSource for CountingFs {
    fn open(&self, path: &Path) -> io::Result<Arc<dyn ByteSource>> {
        Ok(Arc::new(Counted {
            inner: LocalFs.open(path)?,
            read: self.read.clone(),
        }))
    }
    fn metadata(&self, path: &Path) -> io::Result<EntryMeta> {
        LocalFs.metadata(path)
    }
    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        LocalFs.list(dir)
    }
}

/// Bytes read by detection, opening and `info` of `path`, and the file's size.
fn info_bytes(path: &Path) -> (u64, u64) {
    let counting = Arc::new(CountingFs::default());
    let input = Input::new(path, Fs::new(counting.clone()));
    let (_, ds) = registry().open_input(&input).unwrap();
    let info = ds.info().unwrap();
    assert!(info.plane_count > 0 || !info.spectra.is_empty() || !info.traces.is_empty());
    let size = std::fs::metadata(path).unwrap().len();
    (counting.read.load(Ordering::Relaxed), size)
}

fn check(what: &str, path: &Path, budget: u64) {
    let (read, size) = info_bytes(path);
    eprintln!("{what}: info read {read} of {size} bytes (budget {budget})");
    assert!(
        read <= budget,
        "{what}: info read {read} bytes of a {size}-byte file, more than the {budget}-byte budget"
    );
}

#[test]
fn ome_tiff_info_reads_the_header_only() {
    let dir = tempfile::tempdir().unwrap();
    // 64 planes of 1024 x 1024 uint16 (128 MiB uncompressed, deflated on disk).
    let mut img = SyntheticImage::new(1024, 1024, 4, 8, 2);
    let path = dir.path().join("big.ome.tiff");
    write_ome_tiff(&mut img, &path, openreadout_ometiff::Codec::Deflate).unwrap();
    // Detection's 64 KiB, the OME-XML and 64 IFDs.
    check("OME-TIFF", &path, 1 << 20);
}

#[test]
fn mzml_info_reads_spectrum_headers_not_arrays() {
    let dir = tempfile::tempdir().unwrap();
    // 400 spectra of 20,000 points: the arrays are most of the file.
    let path = dir.path().join("big.mzML");
    write_mzml(400, 20_000, &path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    // `info` parses every spectrum header (a few KiB each), never the arrays.
    check("mzML", &path, (64 << 10) + 400 * (16 << 10));
    assert!(
        size > 400 * (100 << 10),
        "the arrays should dominate the file"
    );
}

#[test]
fn spikeglx_info_reads_the_meta_file_only() {
    let dir = tempfile::tempdir().unwrap();
    // 64 channels x 500,000 samples = 61 MiB of samples.
    let bin = write_spikeglx(dir.path(), 64, 500_000).unwrap();
    check("SpikeGLX", &bin, 256 << 10);
}
