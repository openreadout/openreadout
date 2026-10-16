//! Helpers shared by the CBF and SMV readers: opening a plain or gzip-compressed input,
//! guessing the radiation from the wavelength, and an MD5 digest for CBF's `Content-MD5`.

use std::sync::Arc;

use openreadout_core::diffraction::Radiation;
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, Result};

/// Longest wavelength (Å) taken as electrons: X-ray diffraction uses about 0.3–3 Å, electron
/// diffraction at 80–300 kV 0.02–0.04 Å.
pub const ELECTRON_MAX_WAVELENGTH_A: f64 = 0.1;

/// X-rays or electrons, from the wavelength.
pub fn radiation_of(wavelength_a: Option<f64>) -> Option<Radiation> {
    wavelength_a.filter(|w| *w > 0.0).map(|w| {
        if w < ELECTRON_MAX_WAVELENGTH_A {
            Radiation::Electron
        } else {
            Radiation::Xray
        }
    })
}

/// An opened input: the bytes to parse (decompressed when the file is gzip-compressed) and,
/// for gzip files, the compressed size and any problem found while decompressing.
#[derive(Debug)]
pub struct Opened {
    /// The (decompressed) content.
    pub file: SourceFile,
    /// Its length in bytes.
    pub len: u64,
    /// Compressed size and decompression problem of a `.gz` file.
    pub gzip: Option<(u64, Option<String>)>,
}

/// Open `input`, decompressing it once when it is gzip-compressed (`name.cbf.gz`).
pub fn open(input: &Input, format: &'static str) -> Result<Opened> {
    let path = input.path();
    let file = input.open()?;
    let mut head = [0u8; 2];
    let is_gz =
        file.read_exact_at(0, &mut head).is_ok() && openreadout_core::gzip::is_gzip(&head);
    if !is_gz {
        let len = file.size().map_err(|e| Error::io(path, e))?;
        return Ok(Opened {
            file,
            len,
            gzip: None,
        });
    }
    let src = input.fs().source(path).map_err(|e| Error::io(path, e))?;
    let name = path.file_name().map_or_else(
        || "image".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let inner = openreadout_core::gzip::strip_gz(&name).to_string();
    let gz = openreadout_core::gzip::GzipSource::open(src, inner, format)?;
    let summary = gz.summary().clone();
    let file = SourceFile::new(Arc::new(gz));
    let len = file.size().map_err(|e| Error::io(path, e))?;
    Ok(Opened {
        file,
        len,
        gzip: Some((summary.compressed_len, summary.problem.clone())),
    })
}

/// Read `len` bytes at `offset` of `file`, or a corrupt-file error naming what was read.
pub fn read_exact(
    file: &SourceFile,
    offset: u64,
    len: u64,
    file_len: u64,
    format: &'static str,
    what: &str,
) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| Error::corrupt(format, format!("{what}: size overflows")))?;
    if end > file_len {
        return Err(Error::corrupt_at(
            format,
            offset,
            format!(
                "{what} needs {len} bytes at offset {offset} but the file ends at {file_len} (truncated; run `check`)"
            ),
        ));
    }
    let n = usize::try_from(len).map_err(|_| Error::corrupt(format, format!("{what}: too large")))?;
    let mut buf = vec![0u8; n];
    file.read_exact_at(offset, &mut buf)
        .map_err(|e| Error::corrupt_at(format, offset, format!("{what}: {e}")))?;
    Ok(buf)
}

/// A number written in a header, ignoring trailing units and commas.
pub fn number(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches([',', ';', '.']);
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `v` rounded to 12 significant digits, so that unit conversions print as written
/// (0.172 mm, not 0.17200000000000001).
pub fn tidy(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let digits = 11 - v.abs().log10().floor() as i32;
    let scale = 10f64.powi(digits.clamp(-300, 300));
    (v * scale).round() / scale
}

/// MD5 digest (RFC 1321) of `data`.
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64u32)
        .map(|i| ((f64::from(i) + 1.0).sin().abs() * 4_294_967_296.0) as u32)
        .collect();
    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut tail = Vec::with_capacity(128);
    let full = data.len() / 64 * 64;
    tail.extend_from_slice(&data[full..]);
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in data[..full].chunks_exact(64).chain(tail.chunks_exact(64)) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Standard base64 (with padding) of `data`.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= c.len() {
                s.push(char::from(A[((n >> shift) & 63) as usize]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_matches_rfc_1321_vectors() {
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn base64_and_numbers() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(number(" 0.97950 "), Some(0.9795));
        assert_eq!(number("12,"), Some(12.0));
        assert_eq!(number("abc"), None);
        assert_eq!(radiation_of(Some(0.025)), Some(Radiation::Electron));
        assert_eq!(radiation_of(Some(0.98)), Some(Radiation::Xray));
    }
}
