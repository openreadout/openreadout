//! CRAM blocks: the block header (method, content type, content id, sizes, CRC32 from 3.0 on)
//! and the decompression of the block data (CRAM specification §§ 8 and 14).

use std::io::Read;

use super::io::{Cursor, DecodeResult};
use super::rans::MAX_OUT;

/// Block compression methods, by their byte in the block header.
pub(crate) fn method_name(m: u8) -> &'static str {
    match m {
        0 => "raw",
        1 => "gzip",
        2 => "bzip2",
        3 => "lzma",
        4 => "rans4x8",
        5 => "ransNx16",
        6 => "arith",
        7 => "fqzcomp",
        8 => "tok3",
        _ => "unknown",
    }
}

/// Block content types.
pub(crate) const FILE_HEADER: u8 = 0;
pub(crate) const COMPRESSION_HEADER: u8 = 1;
pub(crate) const SLICE_HEADER: u8 = 2;
pub(crate) const EXTERNAL_DATA: u8 = 4;
pub(crate) const CORE_DATA: u8 = 5;

/// One block, decompressed.
#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub(crate) method: u8,
    pub(crate) content_type: u8,
    pub(crate) content_id: i32,
    pub(crate) data: Vec<u8>,
}

/// Decompress bzip2 data expected to hold `len` bytes.
pub(crate) fn bunzip(data: &[u8], len: usize) -> DecodeResult<Vec<u8>> {
    let mut out = Vec::with_capacity(len.min(16 << 20));
    bzip2_rs::DecoderReader::new(data)
        .take(len as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("bzip2: {e}"))?;
    Ok(out)
}

/// Decompress block data with `method` to `raw` bytes.
pub(crate) fn decompress(method: u8, data: &[u8], raw: usize) -> DecodeResult<Vec<u8>> {
    if raw > MAX_OUT {
        return Err(format!("block of {raw} bytes is too large"));
    }
    if raw == 0 {
        return Ok(Vec::new());
    }
    let out = match method {
        0 => data.to_vec(),
        1 => {
            let mut out = Vec::with_capacity(raw.min(16 << 20));
            flate2::read::MultiGzDecoder::new(data)
                .take(raw as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|e| format!("gzip: {e}"))?;
            out
        }
        2 => bunzip(data, raw)?,
        3 => {
            let mut out = Vec::with_capacity(raw.min(16 << 20));
            lzma_rs::xz_decompress(&mut std::io::Cursor::new(data), &mut out)
                .map_err(|e| format!("lzma (xz): {e:?}"))?;
            out
        }
        4 => super::rans::decode_4x8(data)?,
        5 => super::rans::decode_nx16(data, Some(raw))?,
        6 => super::arith::decode(data, Some(raw))?,
        7 => super::fqzcomp::decode(data)?,
        8 => super::tok3::decode(data)?,
        m => return Err(format!("block compression method {m} is not defined")),
    };
    if out.len() != raw {
        return Err(format!(
            "{} block decompresses to {} bytes, its header says {raw}",
            method_name(method),
            out.len()
        ));
    }
    Ok(out)
}

/// Read one block at the cursor; `crc` is true from CRAM 3.0 on. Returns the block and whether
/// its CRC32 matched (None without one).
pub(crate) fn read_block(r: &mut Cursor, crc: bool) -> DecodeResult<(Block, Option<bool>)> {
    let start = r.pos();
    let start_rest = r.rest();
    let method = r.u8()?;
    let content_type = r.u8()?;
    let content_id = r.itf8()?;
    let size = r.itf8_len(MAX_OUT, "block size")?;
    let raw = r.itf8_len(MAX_OUT, "block raw size")?;
    let data = r.take(size)?;
    let crc_ok = if crc {
        let used = r.pos() - start;
        let want = r.u32_le()?;
        Some(crc32fast::hash(&start_rest[..used]) == want)
    } else {
        None
    };
    let data = decompress(method, data, raw)
        .map_err(|e| format!("{} block (content {content_id}): {e}", method_name(method)))?;
    Ok((
        Block {
            method,
            content_type,
            content_id,
            data,
        },
        crc_ok,
    ))
}
