//! The synthetic benchmark inputs must be readable by the real readers and decode to what was
//! encoded; otherwise `cargo bench` without the corpus would measure error paths.

use openreadout_bench::{
    CodecInputs, SyntheticImage, registry, write_mzml, write_ome_tiff, write_spikeglx,
};
use openreadout_codecs as codecs;
use openreadout_core::reader::{Dataset as _, PlaneIndex};

#[test]
fn codec_payloads_round_trip() {
    let inp = CodecInputs::new(96, 64);
    let n = inp.raw.len();
    assert_eq!(codecs::zstd_decode(&inp.zstd, n).unwrap(), inp.raw);
    assert_eq!(
        codecs::zstd1_decode(&inp.zstd1_hilo, n, 2).unwrap(),
        inp.raw
    );
    assert_eq!(codecs::lzw_decode(&inp.lzw, n).unwrap(), inp.raw);
    assert_eq!(codecs::zlib_decode(&inp.zlib, n).unwrap(), inp.raw);
    let j = codecs::jpeg_decode(&inp.jpeg).unwrap();
    assert_eq!((j.width, j.height, j.channels), (96, 64, 3));
    let j2k = codecs::jpeg2000_decode(&inp.jpeg2000).unwrap();
    assert_eq!((j2k.width, j2k.height, j2k.bits_per_sample), (96, 64, 16));
}

#[test]
fn synthetic_files_open_through_the_registry() {
    let reg = registry();
    let dir = tempfile::tempdir().unwrap();

    let mut img = SyntheticImage::new(64, 48, 2, 3, 1);
    let tiff = dir.path().join("s.ome.tiff");
    write_ome_tiff(&mut img, &tiff, openreadout_ometiff::Codec::Deflate).unwrap();
    let (det, mut ds) = reg.open(&tiff).unwrap();
    assert_eq!(det.format_id, "tiff");
    let info = ds.info().unwrap();
    assert_eq!(info.plane_count, 6);
    let p = ds.read_plane(0, PlaneIndex { c: 1, z: 2, t: 0 }).unwrap();
    assert_eq!(
        p.data,
        img.read_plane(0, PlaneIndex { c: 1, z: 2, t: 0 })
            .unwrap()
            .data
    );

    let mzml = dir.path().join("s.mzML");
    write_mzml(5, 100, &mzml).unwrap();
    let (_, mut ds) = reg.open(&mzml).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.spectra[0].scan_count, 5);
    assert_eq!(ds.read_spectrum(0, 4).unwrap().mz.len(), 100);

    let bin = write_spikeglx(dir.path(), 4, 1000).unwrap();
    let (det, mut ds) = reg.open(&bin).unwrap();
    assert_eq!(det.format_id, "spikeglx");
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.channels.len(), t.sample_count), (4, 1000));
    let tr = ds.read_trace(0, 0, 0, 1000).unwrap();
    assert_eq!(tr.channels.len(), 4);
    assert_eq!(tr.channels[0].len(), 1000);
}
