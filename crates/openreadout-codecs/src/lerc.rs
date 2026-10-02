//! LERC2 (Esri Limited Error Raster Compression), as TIFF compression 34887 stores it. Backed by
//! the decoder of `lerc-rs` (Apache-2.0, pure Rust; its decoder has no `unsafe`). The blob's
//! header is read first and the image refused before decoding when it is larger than the
//! caller's limit.

use lerc::bitmask::BitMask;
use lerc::{DataType, SampleData};

use crate::{CodecError, MAX_UNSIZED_OUTPUT, Raster, Result};

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: "lerc",
        detail: detail.into(),
    }
}

/// Validate the header of every blob (bands are concatenated blobs) before lerc-rs sees them:
/// lerc-rs 0.5 divides by the micro-block size and loops over the row and column counts as
/// read, so a zero or negative value there panics or runs for hours (found by fuzz target
/// `codec_lerc`). Header layout (Esri's LERC2, Apache-2.0): `Lerc2 `, i32 version, u32 checksum
/// (version 3+), i32 rows, i32 columns, i32 depth (version 4+), i32 valid pixels, i32 micro-block
/// size, i32 blob size (the whole blob, header included), ...
fn check_headers(data: &[u8]) -> Result<()> {
    let mut rest = data;
    loop {
        let i32_at = |off: usize| -> Result<i32> {
            rest.get(off..off + 4)
                .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| err("truncated header"))
        };
        if !rest.starts_with(b"Lerc2 ") {
            return Err(err("not a LERC2 blob"));
        }
        let version = i32_at(6)?;
        if !(2..=6).contains(&version) {
            return Err(CodecError::Unsupported {
                codec: "lerc",
                detail: format!("LERC2 version {version}"),
            });
        }
        let mut off = if version >= 3 { 14 } else { 10 };
        let (rows, cols) = (i32_at(off)?, i32_at(off + 4)?);
        off += 8;
        let depth = if version >= 4 {
            off += 4;
            i32_at(off - 4)?
        } else {
            1
        };
        let (valid, block, size) = (i32_at(off)?, i32_at(off + 4)?, i32_at(off + 8)?);
        let pixels = i64::from(rows) * i64::from(cols);
        if rows < 1
            || cols < 1
            || depth < 1
            || valid < 0
            || i64::from(valid) > pixels
            || !(1..=1 << 16).contains(&block)
        {
            return Err(err(format!(
                "invalid header: {rows} rows, {cols} columns, depth {depth}, {valid} valid pixels, micro-block size {block}"
            )));
        }
        let size = usize::try_from(size).unwrap_or(0);
        if size < off + 12 || size > rest.len() {
            return Err(err(format!(
                "blob size {size} outside the {} bytes left",
                rest.len()
            )));
        }
        rest = &rest[size..];
        if !rest.starts_with(b"Lerc2 ") {
            return Ok(());
        }
    }
}

/// Decode a LERC blob to interleaved little-endian samples (`depth` values per pixel, bands one
/// after another). Pixels the blob's validity mask marks invalid are set to 0 (what
/// tifffile + imagecodecs return; GDAL substitutes the file's nodata value). The raster's
/// `bits_per_sample`/`float` give the sample type; whether integers are signed is the
/// caller's (TIFF SampleFormat) business.
pub(crate) fn decode(data: &[u8], max_bytes: usize) -> Result<Raster> {
    check_headers(data)?;
    let info = lerc::decode_info(data).map_err(|e| err(e.to_string()))?;
    let bytes_per = match info.data_type {
        DataType::Char | DataType::Byte => 1u64,
        DataType::Short | DataType::UShort => 2,
        DataType::Int | DataType::UInt | DataType::Float => 4,
        DataType::Double => 8,
    };
    let values = [info.height, info.depth.max(1), info.bands.max(1)]
        .into_iter()
        .fold(u64::from(info.width), |acc, n| {
            acc.saturating_mul(u64::from(n))
        });
    if values.saturating_mul(bytes_per) > max_bytes.min(MAX_UNSIZED_OUTPUT) as u64 {
        return Err(err(format!(
            "header declares an implausible {}x{}x{}x{} image",
            info.width, info.height, info.depth, info.bands
        )));
    }
    let img = lerc::decode(data).map_err(|e| err(e.to_string()))?;
    let (w, h) = (img.width as usize, img.height as usize);
    let (depth, bands) = (img.depth.max(1) as usize, img.bands.max(1) as usize);
    let per_band = w * h * depth;
    let data: Vec<u8> = match img.data {
        SampleData::I8(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        SampleData::U8(v) => v,
        SampleData::I16(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        SampleData::U16(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        SampleData::I32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        SampleData::U32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        SampleData::F32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        SampleData::F64(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    };
    let bpv = bytes_per as usize;
    if data.len() != per_band * bands * bpv {
        return Err(err(format!(
            "decoded {} bytes, header declares {}",
            data.len(),
            per_band * bands * bpv
        )));
    }
    let mut data = data;
    for (b, mask) in img.valid_masks.iter().enumerate().take(bands) {
        let BitMask::Explicit(bits) = mask else {
            continue;
        };
        let band = &mut data[b * per_band * bpv..(b + 1) * per_band * bpv];
        for (k, px) in band.chunks_exact_mut(depth * bpv).enumerate() {
            if !bits.get(k).is_some_and(|v| *v) {
                px.fill(0);
            }
        }
    }
    let channels = u32::try_from(depth * bands).map_err(|_| err("too many bands"))?;
    Ok(Raster {
        width: img.width,
        height: img.height,
        channels,
        bits_per_sample: (bytes_per * 8) as u32,
        float: matches!(info.data_type, DataType::Float | DataType::Double),
        bgr: false,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version-4 header: rows, columns, depth, valid pixels, micro-block size; blob size 90.
    fn header(rows: i32, cols: i32, depth: i32, valid: i32, block: i32) -> Vec<u8> {
        let mut b = b"Lerc2 ".to_vec();
        for v in [4, 0, rows, cols, depth, valid, block, 90, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.resize(90, 0);
        b
    }

    /// Headers from the `codec_lerc` fuzz findings: lerc-rs 0.5 panicked (micro-block size 0,
    /// size overflow) or looped for hours (negative dimensions) on such blobs; all are errors.
    #[test]
    fn hostile_headers_are_errors() {
        for h in [
            header(20, 20, 1, 0, 0),
            header(-201_359_616, 1_080_131_839, 1, 0, 8),
            header(845_378_149, 1056, 845_378_048, 0, 8),
            header(20, 20, 1, 401, 8),
            header(0, 0, 0, 0, 0),
        ] {
            assert!(decode(&h, 64 << 20).is_err());
        }
    }
}
