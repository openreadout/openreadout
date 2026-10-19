//! Take one member out of a remote gzip-compressed tar archive by streaming the archive from
//! its start and stopping once the member is written. Some depositors publish only one very
//! large `.tar.gz` (the LINCS L1000 bead files on NCBI GEO are 240 GB); members near the start
//! cost only the bytes before them. The member's size from its header is checked before the
//! file is written.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Bytes of a tar header block.
const BLOCK: usize = 512;

/// Read until `buf` is full; `Ok(false)` at a clean end of stream before any byte.
fn fill(r: &mut impl Read, buf: &mut [u8]) -> Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        let n = r.read(&mut buf[got..])?;
        if n == 0 {
            if got == 0 {
                return Ok(false);
            }
            bail!("tar archive cut short inside a block");
        }
        got += n;
    }
    Ok(true)
}

/// An octal header field (NUL- or space-terminated), or a base-256 size (first byte 0x80).
fn octal(field: &[u8]) -> Result<u64> {
    if field.first() == Some(&0x80) {
        return Ok(field[1..]
            .iter()
            .fold(0u64, |a, &b| (a << 8) | u64::from(b)));
    }
    let s: String = field
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).with_context(|| format!("tar header number {s:?}"))
}

/// The member name of a header block: ustar `prefix/name`, or a GNU long name read before it.
fn header_name(h: &[u8], long: Option<String>) -> String {
    if let Some(l) = long {
        return l;
    }
    let cut = |b: &[u8]| -> String {
        String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())])
            .into_owned()
    };
    let name = cut(&h[0..100]);
    let prefix = if &h[257..262] == b"ustar" {
        cut(&h[345..500])
    } else {
        String::new()
    };
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

/// Copy `n` bytes from `r` to `w` (or discard them when `w` is `None`), then skip the padding
/// to the next block.
fn copy_member(r: &mut impl Read, n: u64, mut w: Option<&mut fs::File>) -> Result<()> {
    let mut left = n;
    let mut buf = vec![0u8; 1 << 16];
    while left > 0 {
        let k = usize::try_from(left.min(buf.len() as u64))?;
        if !fill(r, &mut buf[..k])? {
            bail!("tar archive ends inside a member");
        }
        if let Some(f) = w.as_deref_mut() {
            f.write_all(&buf[..k])?;
        }
        left -= k as u64;
    }
    let pad = usize::try_from((BLOCK as u64 - n % BLOCK as u64) % BLOCK as u64)?;
    if pad > 0 && !fill(r, &mut buf[..pad])? {
        bail!("tar archive ends inside padding");
    }
    Ok(())
}

/// Fetch member `name` of the `.tar.gz` at `url` into `dst`.
pub fn fetch_member(url: &str, name: &str, dst: &Path) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(url).call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    if status != 200 {
        bail!("HTTP {status} for {url}");
    }
    let body = resp.body_mut().as_reader();
    let mut r = flate2::read::MultiGzDecoder::new(body);
    let mut h = [0u8; BLOCK];
    let mut long: Option<String> = None;
    while fill(&mut r, &mut h)? {
        if h.iter().all(|&b| b == 0) {
            break; // end-of-archive blocks
        }
        let size = octal(&h[124..136])?;
        let kind = h[156];
        if kind == b'L' {
            // GNU long name: the next `size` bytes are the name of the following member.
            let mut b = vec![0u8; usize::try_from(size)?];
            if !fill(&mut r, &mut b)? {
                bail!("tar archive ends inside a long name");
            }
            let pad = (BLOCK - b.len() % BLOCK) % BLOCK;
            let mut p = vec![0u8; pad];
            if pad > 0 && !fill(&mut r, &mut p)? {
                bail!("tar archive ends inside padding");
            }
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            long = Some(String::from_utf8_lossy(&b[..end]).into_owned());
            continue;
        }
        let member = header_name(&h, long.take());
        let is_file = kind == b'0' || kind == 0;
        if is_file && member.trim_start_matches("./") == name {
            let tmp = dst.with_extension("part");
            let mut f = fs::File::create(&tmp)?;
            copy_member(&mut r, size, Some(&mut f))?;
            f.flush()?;
            drop(f);
            let got = fs::metadata(&tmp)?.len();
            if got != size {
                bail!("{name}: wrote {got} bytes, the tar header says {size}");
            }
            fs::rename(&tmp, dst)?;
            return Ok(());
        }
        copy_member(&mut r, size, None)?;
    }
    bail!("{name} is not a member of {url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_fields() {
        assert_eq!(octal(b"0000644\0").unwrap(), 0o644);
        assert_eq!(octal(b"00000001750 ").unwrap(), 1000);
        assert_eq!(octal(b"\0\0\0").unwrap(), 0);
        assert_eq!(octal(&[0x80, 0, 0, 1, 0]).unwrap(), 256);
    }
}
