//! XLIF frames stored as image files (TIFF, JPEG, PNG): the *decoded* image of the file is the
//! frame's range of the memory block (`docs/formats/lif.md`, *XLIF frames*). A decoded RGB image
//! is put into the block by channel tag (LAS X keeps its blue, green, red memory order in the
//! XLIF; the image file is ordinary RGB), and an RGB image three times the frame size stands for
//! a grey frame (its first sample).

use std::io::Read;
use std::path::Path;

use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, FormatReader, PlaneIndex, Result};

use crate::FORMAT_ID;

/// Largest decoded frame image we hold in memory.
const MAX_FRAME_BYTES: u64 = 2 << 30;

/// The image-file types a frame can be decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// `.tif` / `.tiff` (a single page).
    Tiff,
    /// `.jpg` / `.jpeg`.
    Jpeg,
    /// `.png`.
    Png,
}

impl FrameKind {
    /// The kind for a lower-case file extension, if we decode it.
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "tif" | "tiff" => Some(FrameKind::Tiff),
            "jpg" | "jpeg" => Some(FrameKind::Jpeg),
            "png" => Some(FrameKind::Png),
            _ => None,
        }
    }

    /// Short name (`tiff`, `jpeg`, `png`).
    pub fn name(self) -> &'static str {
        match self {
            FrameKind::Tiff => "tiff",
            FrameKind::Jpeg => "jpeg",
            FrameKind::Png => "png",
        }
    }
}

/// How one frame file becomes memory-block bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDecode {
    pub kind: FrameKind,
    /// The frame's `Size` in the XLIF.
    pub size: u64,
    /// For RGB frames: `remap[p]` = the image sample that goes to memory sample position `p`
    /// (from the channels' tags and `BytesInc`); `None` keeps the image's order.
    pub remap: Option<Vec<usize>>,
}

/// A decoded image: interleaved little-endian samples.
struct Decoded {
    data: Vec<u8>,
    spp: usize,
    bps: usize,
}

fn unsupported(what: String) -> Error {
    Error::unsupported(
        FORMAT_ID,
        what,
        "The XLIF frame image could not be used as its memory block; convert the experiment to .lif in LAS X, or read the frame image with the TIFF/JPEG/PNG reader.",
    )
}

fn decode(fs: &Fs, path: &Path, kind: FrameKind) -> Result<Decoded> {
    let len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
    if len > MAX_FRAME_BYTES {
        return Err(unsupported(format!("a {len}-byte frame image")));
    }
    match kind {
        FrameKind::Tiff => {
            let mut ds = openreadout_tiff::TiffReader
                .open_input(&Input::new(path.to_path_buf(), fs.clone()))?;
            let info = ds.info()?;
            if info.images.len() != 1 || info.plane_count != 1 {
                return Err(unsupported(format!(
                    "a frame TIFF with {} image(s) and {} plane(s) (only single-page frames are read)",
                    info.images.len(),
                    info.plane_count
                )));
            }
            let p = ds.read_plane(0, PlaneIndex::default())?;
            Ok(Decoded {
                spp: p.samples_per_pixel.max(1) as usize,
                bps: p.pixel_type.bytes_per_sample(),
                data: p.data,
            })
        }
        FrameKind::Jpeg => {
            let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
            let r = openreadout_codecs::jpeg_decode_limited(
                &bytes,
                usize::try_from(MAX_FRAME_BYTES).unwrap_or(usize::MAX),
            )
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("{}: {e}", path.display())))?;
            Ok(Decoded {
                spp: r.channels as usize,
                bps: (r.bits_per_sample as usize).div_ceil(8).max(1),
                data: r.data,
            })
        }
        FrameKind::Png => {
            let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).map_err(|e| Error::io(path, e))?;
            let bad = |e: String| Error::corrupt(FORMAT_ID, format!("{}: {e}", path.display()));
            let dec = png::Decoder::new(std::io::Cursor::new(&bytes));
            let mut r = dec.read_info().map_err(|e| bad(e.to_string()))?;
            let n = r
                .output_buffer_size()
                .filter(|n| (*n as u64) <= MAX_FRAME_BYTES)
                .ok_or_else(|| unsupported("a PNG frame larger than 2 GiB".into()))?;
            let mut buf = vec![0u8; n];
            let fi = r.next_frame(&mut buf).map_err(|e| bad(e.to_string()))?;
            buf.truncate(fi.buffer_size());
            let spp = match fi.color_type {
                png::ColorType::Grayscale => 1,
                png::ColorType::Rgb => 3,
                other => return Err(unsupported(format!("a {other:?} PNG frame"))),
            };
            let bps = match fi.bit_depth {
                png::BitDepth::Eight => 1,
                png::BitDepth::Sixteen => {
                    // PNG stores 16-bit samples big-endian.
                    for s in buf.as_chunks_mut::<2>().0 {
                        s.swap(0, 1);
                    }
                    2
                }
                other => return Err(unsupported(format!("a {other:?}-bit PNG frame"))),
            };
            Ok(Decoded {
                data: buf,
                spp,
                bps,
            })
        }
    }
}

/// The memory-block bytes of one frame: its image decoded and fitted to the XLIF's `Size`.
pub(crate) fn frame_bytes(fs: &Fs, path: &Path, spec: &FrameDecode) -> Result<Vec<u8>> {
    let d = decode(fs, path, spec.kind)?;
    let n = d.data.len() as u64;
    if n == spec.size {
        if d.spp == 3
            && let Some(map) = &spec.remap
        {
            let px = 3 * d.bps;
            let mut out = vec![0u8; d.data.len()];
            for (o, i) in out.chunks_exact_mut(px).zip(d.data.chunks_exact(px)) {
                for (pos, &src) in map.iter().enumerate() {
                    o[pos * d.bps..(pos + 1) * d.bps]
                        .copy_from_slice(&i[src * d.bps..(src + 1) * d.bps]);
                }
            }
            return Ok(out);
        }
        return Ok(d.data);
    }
    // A grey frame saved as an RGB image: its first sample.
    if d.spp == 3 && n == spec.size.saturating_mul(3) {
        let px = 3 * d.bps;
        return Ok(d
            .data
            .chunks_exact(px)
            .flat_map(|p| p[..d.bps].iter().copied())
            .collect());
    }
    Err(Error::corrupt(
        FORMAT_ID,
        format!(
            "frame image {} decodes to {n} bytes ({} sample(s) of {} byte(s) per pixel), the XLIF frame holds {}",
            path.display(),
            d.spp,
            d.bps,
            spec.size
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(FrameKind::from_ext("tif"), Some(FrameKind::Tiff));
        assert_eq!(FrameKind::from_ext("jpeg"), Some(FrameKind::Jpeg));
        assert_eq!(FrameKind::from_ext("bmp"), None);
        assert_eq!(FrameKind::Png.name(), "png");
    }

    #[test]
    fn png_frames_are_remapped_and_greyed() {
        let dir = std::env::temp_dir().join(format!("lif-frames-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.png");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut e = png::Encoder::new(std::io::BufWriter::new(f), 2, 1);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[1, 2, 3, 4, 5, 6]).unwrap();
        }
        let fs = Fs::local();
        // memory order blue, green, red: position 0 takes the image's sample 2
        let spec = FrameDecode {
            kind: FrameKind::Png,
            size: 6,
            remap: Some(vec![2, 1, 0]),
        };
        assert_eq!(frame_bytes(&fs, &p, &spec).unwrap(), [3, 2, 1, 6, 5, 4]);
        let grey = FrameDecode {
            kind: FrameKind::Png,
            size: 2,
            remap: None,
        };
        assert_eq!(frame_bytes(&fs, &p, &grey).unwrap(), [1, 4]);
        let wrong = FrameDecode {
            kind: FrameKind::Png,
            size: 5,
            remap: None,
        };
        assert!(frame_bytes(&fs, &p, &wrong).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
