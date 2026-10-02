//! Content fingerprints (cheap, for moves and duplicate candidates) and full content hashes
//! (for confirming duplicates).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::Xxh3;

use crate::record::Member;

/// Bytes read from each end of a file for its fingerprint (and from the start for detection).
pub const SPAN: u64 = 64 * 1024;

fn read_up_to(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = f.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// The first [`SPAN`] bytes of a file.
pub fn read_head(path: &Path) -> std::io::Result<(File, Vec<u8>)> {
    let mut f = File::open(path)?;
    let mut head = vec![0u8; usize::try_from(SPAN).unwrap_or(65_536)];
    let n = read_up_to(&mut f, &mut head)?;
    head.truncate(n);
    Ok((f, head))
}

/// Fingerprint of a file of `size` bytes whose head was already read: xxh3-128 of a version
/// tag, the size, the head and the last [`SPAN`] bytes (the tail is read only when the file is
/// longer than the head). Returns the fingerprint and the extra bytes read.
pub fn fingerprint_file(f: &mut File, size: u64, head: &[u8]) -> std::io::Result<(String, u64)> {
    let mut h = Xxh3::new();
    h.update(b"openreadout-fp1");
    h.update(&size.to_le_bytes());
    h.update(head);
    let head_len = head.len() as u64;
    let mut extra = 0u64;
    if size > head_len {
        let start = size.saturating_sub(SPAN).max(head_len);
        let mut tail = vec![0u8; usize::try_from(size - start).unwrap_or(0)];
        f.seek(SeekFrom::Start(start))?;
        let n = read_up_to(f, &mut tail)?;
        tail.truncate(n);
        extra = n as u64;
        h.update(&tail);
    }
    Ok((format!("{:032x}", h.digest128()), extra))
}

/// Fingerprint of a directory data set: every member's path (relative to `dir`) and size, and
/// the fingerprint of its largest file. Returns the fingerprint and the bytes read.
pub fn fingerprint_dir(dir: &Path, members: &[Member]) -> std::io::Result<(String, u64)> {
    let mut h = Xxh3::new();
    h.update(b"openreadout-fpdir1");
    let base = dir.to_string_lossy();
    for m in members {
        let rel = m.path.strip_prefix(base.as_ref()).unwrap_or(&m.path);
        h.update(rel.as_bytes());
        h.update(&[0]);
        h.update(&m.size.to_le_bytes());
    }
    let mut read = 0u64;
    if let Some(big) = members.iter().max_by_key(|m| m.size) {
        let p = Path::new(&big.path);
        let (mut f, head) = read_head(p)?;
        let (fp, extra) = fingerprint_file(&mut f, big.size, &head)?;
        read = head.len() as u64 + extra;
        h.update(fp.as_bytes());
    }
    Ok((format!("{:032x}", h.digest128()), read))
}

/// SHA-256 of the file's content (hex), and the bytes read.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((hex(&h.finalize()), total))
}

/// SHA-256 over a directory data set: each member's relative path, a NUL and its content
/// SHA-256, in path order.
pub fn sha256_dir(dir: &Path, members: &[Member]) -> std::io::Result<(String, u64)> {
    let mut h = Sha256::new();
    let base = dir.to_string_lossy();
    let mut total = 0u64;
    for m in members {
        let rel = m.path.strip_prefix(base.as_ref()).unwrap_or(&m.path);
        let (d, n) = sha256_file(Path::new(&m.path))?;
        total += n;
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update(d.as_bytes());
    }
    Ok((hex(&h.finalize()), total))
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_depends_on_ends_and_size_not_middle() {
        let d = tempfile::tempdir().unwrap();
        let mut a = vec![7u8; 300_000];
        let p = d.path().join("a");
        std::fs::write(&p, &a).unwrap();
        let fp = |p: &Path| {
            let (mut f, head) = read_head(p).unwrap();
            let size = std::fs::metadata(p).unwrap().len();
            fingerprint_file(&mut f, size, &head).unwrap()
        };
        let (f1, extra) = fp(&p);
        assert_eq!(extra, SPAN);
        a[150_000] = 8; // the middle is not read
        std::fs::write(&p, &a).unwrap();
        assert_eq!(fp(&p).0, f1);
        a[299_999] = 8;
        std::fs::write(&p, &a).unwrap();
        assert_ne!(fp(&p).0, f1);
        let small = d.path().join("s");
        std::fs::write(&small, b"hello").unwrap();
        assert_eq!(fp(&small).1, 0);
        assert_eq!(
            sha256_file(&small).unwrap().0,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
