//! Download one member of a remote zip archive with HTTP range requests, so a corpus entry can
//! take a single file out of a large deposited archive (Zenodo serves ranges) without fetching
//! the whole archive. Stored and deflated members, zip64 sizes and offsets; the member's CRC-32
//! and size are verified before the file is written.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

/// Bytes `[start, end]` (inclusive) of `url`.
fn range(url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    let mut resp = agent()
        .get(url)
        .header("Range", &format!("bytes={start}-{end}"))
        .call()
        .with_context(|| format!("GET {url} (range {start}-{end})"))?;
    let status = resp.status().as_u16();
    if status != 206 {
        bail!("HTTP {status} for a range request to {url} (the server must serve byte ranges)");
    }
    let mut out = Vec::new();
    resp.body_mut().as_reader().read_to_end(&mut out)?;
    Ok(out)
}

fn total_len(url: &str) -> Result<u64> {
    // A one-byte range reports the total length in Content-Range: bytes 0-0/<total>.
    let resp = agent()
        .get(url)
        .header("Range", "bytes=0-0")
        .call()
        .with_context(|| format!("GET {url}"))?;
    let cr = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    cr.rsplit('/')
        .next()
        .and_then(|s| s.trim().parse().ok())
        .with_context(|| format!("no total length in Content-Range {cr:?} of {url}"))
}

fn le(b: &[u8], at: usize, n: usize) -> Result<u64> {
    let s = b.get(at..at + n).context("zip structure cut short")?;
    Ok(s.iter().rev().fold(0u64, |a, &x| (a << 8) | u64::from(x)))
}

/// Fetch member `name` of the zip at `url` into `dst`.
pub fn fetch_member(url: &str, name: &str, dst: &Path) -> Result<()> {
    let n = total_len(url)?;
    let tail_len = n.min(128 * 1024);
    let tail = range(url, n - tail_len, n - 1)?;
    let eocd = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .context("no end-of-central-directory record")?;
    let mut cd_size = le(&tail, eocd + 12, 4)?;
    let mut cd_off = le(&tail, eocd + 16, 4)?;
    if (cd_off == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF)
        && let Some(z) = tail.windows(4).rposition(|w| w == b"PK\x06\x06")
    {
        cd_size = le(&tail, z + 40, 8)?;
        cd_off = le(&tail, z + 48, 8)?;
    }
    let cd = range(url, cd_off, cd_off + cd_size - 1)?;
    let mut p = 0usize;
    while cd.get(p..p + 4) == Some(b"PK\x01\x02") {
        let method = le(&cd, p + 10, 2)?;
        let crc = le(&cd, p + 16, 4)? as u32;
        let mut csize = le(&cd, p + 20, 4)?;
        let mut usize_ = le(&cd, p + 24, 4)?;
        let nlen = le(&cd, p + 28, 2)? as usize;
        let xlen = le(&cd, p + 30, 2)? as usize;
        let clen = le(&cd, p + 32, 2)? as usize;
        let mut loff = le(&cd, p + 42, 4)?;
        let ename = String::from_utf8_lossy(cd.get(p + 46..p + 46 + nlen).context("name")?);
        if ename == name {
            let extra = cd
                .get(p + 46 + nlen..p + 46 + nlen + xlen)
                .context("extra")?;
            let mut q = 0usize;
            while q + 4 <= extra.len() {
                let id = le(extra, q, 2)?;
                let len = le(extra, q + 2, 2)? as usize;
                if id == 1 {
                    let mut k = q + 4;
                    for field in [&mut usize_, &mut csize, &mut loff] {
                        if *field == 0xFFFF_FFFF {
                            *field = le(extra, k, 8)?;
                            k += 8;
                        }
                    }
                }
                q += 4 + len;
            }
            let lh = range(url, loff, loff + 29)?;
            if lh.get(0..4) != Some(b"PK\x03\x04") {
                bail!("{name}: no local header at {loff}");
            }
            let start = loff + 30 + le(&lh, 26, 2)? + le(&lh, 28, 2)?;
            let raw = range(url, start, start + csize - 1)?;
            let data = match method {
                0 => raw,
                8 => {
                    let mut out = Vec::with_capacity(usize::try_from(usize_).unwrap_or(0));
                    flate2::read::DeflateDecoder::new(&raw[..]).read_to_end(&mut out)?;
                    out
                }
                m => bail!("{name}: zip compression method {m} is not supported"),
            };
            if data.len() as u64 != usize_ {
                bail!(
                    "{name}: {} bytes after decompression, expected {usize_}",
                    data.len()
                );
            }
            let mut h = flate2::Crc::new();
            h.update(&data);
            if h.sum() != crc {
                bail!("{name}: CRC-32 mismatch");
            }
            let tmp = dst.with_extension("part");
            fs::File::create(&tmp)?.write_all(&data)?;
            fs::rename(&tmp, dst)?;
            return Ok(());
        }
        p += 46 + nlen + xlen + clen;
    }
    bail!("{name} is not a member of {url}")
}
