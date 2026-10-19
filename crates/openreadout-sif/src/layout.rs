//! Andor SIF file structure: the signature, one header-and-data block per data type (signal,
//! then reference, background, live and source when present), and the XML trailer of newer
//! versions. A block is a text header of space-separated values with a few binary bytes, the
//! axis names, the image structure (frames, sub-images, binning), one time stamp per frame, then
//! float32 samples. Layout and vocabulary: `docs/formats/andor-sif.md`.
//!
//! The header is read from a buffer (`MAX_HEADER` bytes per block at most); every position is
//! bounds-checked and a malformed header becomes an error, never a panic.

use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// The first 36 bytes of every SIF file.
pub const SIF_MAGIC: &[u8] = b"Andor Technology Multi-Channel File\n";
/// Bytes of a block header read at most (the header before the data).
pub const MAX_HEADER: u64 = 1 << 20;
/// Bytes of the XML trailer read at most.
pub const MAX_XML: u64 = 4 << 20;
/// Frames accepted at most.
pub const MAX_FRAMES: u64 = 10_000_000;
/// Sub-images accepted at most.
pub const MAX_SUBIMAGES: u64 = 4096;
/// Names of the data blocks, in file order.
pub const BLOCK_NAMES: [&str; 5] = ["signal", "reference", "background", "live", "source"];

/// True when `head` starts with the SIF signature.
pub fn looks_like_sif(head: &[u8]) -> bool {
    head.starts_with(SIF_MAGIC)
}

/// One sub-image (a region of the detector, read with a binning).
#[derive(Debug, Clone, PartialEq)]
pub struct SubImage {
    /// Leftmost detector column (1-based).
    pub left: i64,
    /// Top detector row.
    pub top: i64,
    /// Rightmost detector column.
    pub right: i64,
    /// Bottom detector row.
    pub bottom: i64,
    /// Rows binned together.
    pub y_binning: i64,
    /// Columns binned together.
    pub x_binning: i64,
    /// Width in (binned) pixels.
    pub width: u64,
    /// Height in (binned) pixels.
    pub height: u64,
}

/// One data block (signal, background, …).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Which block: an index into `BLOCK_NAMES`.
    pub role: usize,
    /// File offset of the block's first header line.
    pub start: u64,
    /// SIF version on the block's first line (65555 … 65567).
    pub version: u64,
    /// Acquisition start, Unix seconds.
    pub acquired_unix: i64,
    /// Detector temperature, °C (-999 when not recorded).
    pub temperature_c: f64,
    /// Exposure time, s.
    pub exposure_s: f64,
    /// Accumulation cycle time, s.
    pub accumulation_cycle_s: f64,
    /// Accumulated cycle time, s.
    pub accumulated_cycle_s: f64,
    /// Number of accumulations per frame.
    pub accumulations: i64,
    /// Time between frames (stack cycle time), s.
    pub kinetic_cycle_s: f64,
    /// Pixel readout time, s.
    pub readout_s: f64,
    /// The values of the first line after these, as written.
    pub line3_rest: Vec<String>,
    /// Detector model (`DU888_BV`).
    pub detector: String,
    /// Detector size in pixels: columns, rows.
    pub detector_size: (u64, u64),
    /// The file name the acquisition software saved to.
    pub original_name: String,
    /// User text (calibrations per frame, comments), printable part.
    pub user_text: String,
    /// Wavelength calibration: coefficients in ascending powers of the 1-based pixel number.
    pub calibration: Vec<f64>,
    /// Per-frame calibrations from the user text, frame order.
    pub frame_calibrations: Vec<Vec<f64>>,
    /// Text lines between the shutter line and the axis names, as written (printable only).
    pub settings_lines: Vec<String>,
    /// Name of the frame (X) axis (`Wavelength`, `Pixel number`, `Raman shift`).
    pub frame_axis: String,
    /// What the values are (`Counts`, `Photons`).
    pub data_type: String,
    /// Name of the image (Y) axis.
    pub image_axis: String,
    /// Detector area of the acquisition: left, top, right, bottom.
    pub area: [i64; 4],
    /// Frames.
    pub frames: u64,
    /// Pixels of one frame (all sub-images).
    pub frame_pixels: u64,
    /// The sub-images, stacked in Y in this order.
    pub sub_images: Vec<SubImage>,
    /// One time stamp per frame, µs since the first.
    pub frame_times_us: Vec<i64>,
    /// File offset of the first sample.
    pub data_offset: u64,
}

impl Block {
    /// Width of a frame (the widest sub-image).
    pub fn width(&self) -> u64 {
        self.sub_images.iter().map(|s| s.width).max().unwrap_or(0)
    }
    /// Height of a frame (sub-images stacked).
    pub fn height(&self) -> u64 {
        self.sub_images.iter().map(|s| s.height).sum()
    }
    /// Bytes of one frame.
    pub fn frame_bytes(&self) -> u64 {
        self.frame_pixels * 4
    }
    /// One past the last sample.
    pub fn data_end(&self) -> u64 {
        self.data_offset + self.frames * self.frame_bytes()
    }
    /// The block's name (`signal`, `background`, …).
    pub fn name(&self) -> &'static str {
        BLOCK_NAMES.get(self.role).copied().unwrap_or("block")
    }
    /// True when the calibration maps pixel `p` to `p` (no calibration).
    pub fn uncalibrated(&self) -> bool {
        let c = &self.calibration;
        c.is_empty()
            || (c.first() == Some(&0.0)
                && c.get(1) == Some(&1.0)
                && c.iter().skip(2).all(|x| *x == 0.0))
    }
    /// The calibrated X value of 0-based column `i` (`None` without a calibration).
    pub fn x_value(&self, i: u64) -> Option<f64> {
        if self.uncalibrated() {
            return None;
        }
        let p = (i + 1) as f64;
        Some(
            self.calibration
                .iter()
                .rev()
                .fold(0.0, |acc, c| acc * p + c),
        )
    }
}

/// The parsed file.
#[derive(Debug, Clone, PartialEq)]
pub struct SifLayout {
    /// File length.
    pub file_len: u64,
    /// The data blocks, signal first.
    pub blocks: Vec<Block>,
    /// The XML trailer, as text.
    pub xml: Option<String>,
    /// Bytes after the last structure the reader knows.
    pub trailing_bytes: u64,
    /// Problems found while parsing (warnings).
    pub problems: Vec<String>,
}

/// A cursor over a header buffer that starts at file offset `origin`.
struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
    origin: u64,
}

impl<'a> Cur<'a> {
    fn err(&self, what: impl Into<String>) -> Error {
        Error::corrupt_at(
            FORMAT_ID,
            self.origin + self.pos as u64,
            format!("SIF header: {}", what.into()),
        )
    }
    /// The next space-delimited token (skipping leading spaces, stopping at a space or newline).
    fn token(&mut self) -> Result<&'a [u8]> {
        while self.b.get(self.pos) == Some(&b' ') {
            self.pos += 1;
        }
        let start = self.pos;
        while let Some(c) = self.b.get(self.pos) {
            if *c == b' ' || *c == b'\n' {
                break;
            }
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.err("a value is missing"));
        }
        let t = &self.b[start..self.pos];
        if self.b.get(self.pos) == Some(&b' ') {
            self.pos += 1;
        }
        Ok(t)
    }
    fn int(&mut self) -> Result<i64> {
        let t = self.token()?;
        std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| {
                self.err(format!(
                    "`{}` is not an integer",
                    String::from_utf8_lossy(t)
                ))
            })
    }
    fn float(&mut self) -> Result<f64> {
        let t = self.token()?;
        std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| self.err(format!("`{}` is not a number", String::from_utf8_lossy(t))))
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        if self.pos + n > self.b.len() {
            return Err(self.err("the header is cut short"));
        }
        self.pos += n;
        Ok(())
    }
    /// The rest of the current line (without the newline), consumed.
    fn line(&mut self) -> Result<&'a [u8]> {
        let rest = self.b.get(self.pos..).unwrap_or_default();
        let n = rest
            .iter()
            .position(|c| *c == b'\n')
            .ok_or_else(|| self.err("a line has no end"))?;
        self.pos += n + 1;
        Ok(&rest[..n])
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .b
            .get(
                self.pos
                    ..self
                        .pos
                        .checked_add(n)
                        .ok_or_else(|| self.err("length overflows"))?,
            )
            .ok_or_else(|| self.err(format!("{n} bytes run past the header")))?;
        self.pos += n;
        Ok(s)
    }
}

fn printable(b: &[u8]) -> String {
    b.iter()
        .filter(|c| (32..127).contains(*c) || **c == b'\n' || **c == b'\t' || **c == b'\r')
        .map(|c| *c as char)
        .collect::<String>()
}

fn is_digits(b: &[u8]) -> bool {
    let t = b.trim_ascii();
    !t.is_empty() && t.iter().all(u8::is_ascii_digit)
}

/// A decimal number terminated by a newline at `p`: (value, position after the newline).
fn number_line(b: &[u8], p: usize) -> Option<(u64, usize)> {
    let rest = b.get(p..)?;
    let n = rest.iter().take(24).position(|c| *c == b'\n')?;
    let t = &rest[..n];
    if !is_digits(t) {
        return None;
    }
    Some((std::str::from_utf8(t).ok()?.trim().parse().ok()?, p + n + 1))
}

/// The axis names and the image-structure line at `p`, if they start there: three
/// length-prefixed printable strings, then a line of at least nine integers starting with 655xx.
#[allow(clippy::many_single_char_names)]
fn axes_at(b: &[u8], p: usize) -> Option<([String; 3], Vec<i64>, usize)> {
    let mut q = p;
    let mut names: [String; 3] = Default::default();
    for name in &mut names {
        let (n, after) = number_line(b, q)?;
        let n = usize::try_from(n).ok().filter(|n| (1..=255).contains(n))?;
        let s = b.get(after..after + n)?;
        if !s.iter().all(|c| (32..127).contains(c)) {
            return None;
        }
        *name = String::from_utf8_lossy(s).trim().to_string();
        q = after + n;
    }
    let rest = b.get(q..)?;
    let n = rest.iter().take(256).position(|c| *c == b'\n')?;
    let line = &rest[..n];
    let vals: Vec<i64> = line
        .split(|c| *c == b' ')
        .filter(|t| !t.is_empty())
        .map(|t| std::str::from_utf8(t).ok()?.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if vals.len() < 9 || !(65536..=65599).contains(&vals[0]) {
        return None;
    }
    Some((names, vals, q + n + 1))
}

fn floats(line: &[u8]) -> Option<Vec<f64>> {
    let v: Vec<f64> = line
        .split(|c| *c == b' ' || *c == b',' || *c == b'\t' || *c == b'\r')
        .filter(|t| !t.is_empty())
        .map(|t| std::str::from_utf8(t).ok()?.parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    (!v.is_empty() && v.iter().all(|x| x.is_finite())).then_some(v)
}

/// Parse one block whose first line starts at `start`; `limit` is the file length.
#[allow(clippy::many_single_char_names, clippy::too_many_lines)]
fn parse_block(f: &SourceFile, start: u64, file_len: u64, role: usize) -> Result<Block> {
    let n = file_len.saturating_sub(start).min(MAX_HEADER);
    let mut buf = vec![0u8; usize::try_from(n).unwrap_or(0)];
    f.read_exact_at(start, &mut buf).map_err(|e| {
        Error::corrupt_at(FORMAT_ID, start, format!("cannot read a SIF header: {e}"))
    })?;
    let mut c = Cur {
        b: &buf,
        pos: 0,
        origin: start,
    };
    // first line: version, three values, time, temperature, 10 bytes, then the timing values
    let version = u64::try_from(c.int()?).map_err(|_| c.err("negative version"))?;
    if !(65536..=65599).contains(&version) {
        return Err(c.err(format!("SIF version {version} is not a 655xx value")));
    }
    for _ in 0..3 {
        c.token()?;
    }
    let acquired_unix = c.int()?;
    let temperature_c = c.float()?;
    c.skip(10)?;
    c.token()?;
    let exposure_s = c.float()?;
    let accumulation_cycle_s = c.float()?;
    let accumulated_cycle_s = c.float()?;
    let accumulations = c.int()?;
    c.skip(2)?;
    let kinetic_cycle_s = c.float()?;
    let readout_s = c.float()?;
    // the end of the first line: the newline followed by the detector line and a size line
    let mut end = None;
    let mut p = c.pos;
    while let Some(i) = buf
        .get(p..)
        .and_then(|r| r.iter().take(4096).position(|x| *x == b'\n'))
    {
        let nl = p + i;
        let det_end = buf
            .get(nl + 1..)
            .and_then(|r| r.iter().take(128).position(|x| *x == b'\n'))
            .map(|k| nl + 1 + k);
        if let Some(de) = det_end {
            let det = &buf[nl + 1..de];
            let size_line = buf
                .get(de + 1..)
                .and_then(|r| r.iter().take(64).position(|x| *x == b'\n').map(|k| &r[..k]));
            let shaped = !det.is_empty()
                && det.iter().all(|x| (32..127).contains(x))
                && size_line.is_some_and(|s| {
                    s.first() == Some(&b' ')
                        && s.split(|x| *x == b' ')
                            .filter(|t| !t.is_empty())
                            .filter(|t| is_digits(t))
                            .count()
                            == 3
                });
            if shaped {
                end = Some(nl);
                break;
            }
        }
        p = nl + 1;
    }
    let end = end.ok_or_else(|| c.err("no detector line after the first header line"))?;
    let line3_rest = buf[c.pos.min(end)..end]
        .split(|x| *x == b' ')
        .filter(|t| !t.is_empty() && t.iter().all(|x| (33..127).contains(x)))
        .map(|t| String::from_utf8_lossy(t).to_string())
        .collect();
    c.pos = end + 1;
    let detector = printable(c.line()?).trim().to_string();
    let w = c.int()?;
    let h = c.int()?;
    let name_len = c.int()?;
    if c.b.get(c.pos) == Some(&b'\n') {
        c.pos += 1;
    }
    let name_len = usize::try_from(name_len).map_err(|_| c.err("negative file-name length"))?;
    let original_name = printable(c.bytes(name_len)?).trim().to_string();
    c.line()?; // the rest of the file-name line
    // user text
    c.token()?;
    let user_len = c.int()?;
    if c.b.get(c.pos) == Some(&b'\n') {
        c.pos += 1;
    }
    let user_len = usize::try_from(user_len).map_err(|_| c.err("negative user-text length"))?;
    let user = c.bytes(user_len)?;
    let user_text = printable(openreadout_core::bytes::until_nul(user))
        .trim()
        .to_string();
    c.line()?; // newline after the user text
    c.line()?; // shutter line
    let settings_start = c.pos;
    // the axis names and the image structure: found by their shape
    let mut found = None;
    let mut p = c.pos;
    while p < buf.len() {
        if let Some(x) = axes_at(&buf, p) {
            found = Some((p, x));
            break;
        }
        match buf[p..].iter().position(|x| *x == b'\n') {
            Some(i) => p += i + 1,
            None => break,
        }
    }
    let Some((axes_start, (names, s, after))) = found else {
        return Err(c.err("no axis names and image structure found"));
    };
    // settings lines; calibration = the line after the last calibration-version line
    let settings: Vec<&[u8]> = buf[settings_start..axes_start]
        .split(|x| *x == b'\n')
        .collect();
    let mut calibration = Vec::new();
    for (k, l) in settings.iter().enumerate() {
        let toks: Vec<&[u8]> = l.split(|x| *x == b' ').filter(|t| !t.is_empty()).collect();
        let versioned = toks
            .first()
            .is_some_and(|t| t.starts_with(b"655") && t.len() == 5);
        let control = toks.get(1).is_some_and(|t| t.len() == 1 && t[0] < 32);
        if versioned
            && control
            && let Some(v) = settings.get(k + 1).and_then(|l| floats(l))
        {
            calibration = v;
        }
    }
    let settings_lines = settings
        .iter()
        .map(|l| printable(l).trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut frame_calibrations = Vec::new();
    for l in user_text.lines() {
        if let Some(rest) = l.strip_prefix("Calibration data for frame ")
            && let Some((_, v)) = rest.split_once(':')
            && let Some(v) = floats(v.as_bytes())
        {
            frame_calibrations.push(v);
        }
    }
    let [frame_axis, data_type, image_axis] = names;
    let area = [s[1], s[2], s[3], s[4]];
    let frames = u64::try_from(s[5]).map_err(|_| c.err("negative frame count"))?;
    let n_sub = u64::try_from(s[6]).map_err(|_| c.err("negative sub-image count"))?;
    let total = u64::try_from(s[7]).map_err(|_| c.err("negative pixel count"))?;
    let frame_pixels = u64::try_from(s[8]).map_err(|_| c.err("negative pixel count"))?;
    if frames > MAX_FRAMES || n_sub == 0 || n_sub > MAX_SUBIMAGES {
        return Err(c.err(format!(
            "{frames} frames of {n_sub} sub-images are outside what this reader accepts"
        )));
    }
    if frames.checked_mul(frame_pixels) != Some(total) {
        return Err(c.err(format!(
            "{frames} frames of {frame_pixels} pixels do not make the {total} pixels the header declares"
        )));
    }
    c.pos = after;
    let mut sub_images = Vec::new();
    for _ in 0..n_sub {
        let line = c.line()?;
        let v: Vec<i64> = line
            .split(|x| *x == b' ')
            .filter(|t| !t.is_empty())
            .filter_map(|t| std::str::from_utf8(t).ok()?.parse().ok())
            .collect();
        let [_, left, top, right, bottom, yb, xb, ..] = v[..] else {
            return Err(c.err("a sub-image line has fewer than 7 values"));
        };
        if xb < 1 || yb < 1 || right < left || top < bottom {
            return Err(c.err(format!(
                "sub-image area {left}..{right} × {bottom}..{top} with binning {xb} × {yb} is not usable"
            )));
        }
        sub_images.push(SubImage {
            left,
            top,
            right,
            bottom,
            y_binning: yb,
            x_binning: xb,
            width: ((1 + right - left) / xb) as u64,
            height: ((1 + top - bottom) / yb) as u64,
        });
    }
    let sum: u64 = sub_images.iter().map(|s| s.width * s.height).sum();
    if sum != frame_pixels {
        return Err(c.err(format!(
            "the sub-images hold {sum} pixels; the header declares {frame_pixels} per frame"
        )));
    }
    let mut frame_times_us = Vec::new();
    for _ in 0..frames.min(MAX_FRAMES) {
        let l = c.line()?;
        let v = std::str::from_utf8(l)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .ok_or_else(|| c.err("a frame time stamp is not an integer"))?;
        frame_times_us.push(v);
    }
    // where the data starts: right here, after a `0` line, or after a `1` line and one more
    // number per frame; the right one ends the data where a flag line (or the file) begins
    let here = c.pos;
    let mut candidates = vec![here];
    if let Some((flag, after)) = number_line(&buf, here) {
        if flag == 0 {
            candidates.insert(0, after);
        } else if flag == 1 {
            let mut q = after;
            let mut ok = true;
            for _ in 0..frames {
                if let Some((_, a)) = number_line(&buf, q) {
                    q = a;
                } else {
                    ok = false;
                    break;
                }
            }
            if ok {
                candidates.insert(0, q);
            }
        }
    }
    let data_bytes = total
        .checked_mul(4)
        .ok_or_else(|| c.err("data size overflows"))?;
    let mut data_offset = None;
    for cand in &candidates {
        let off = start + *cand as u64;
        let end = off.saturating_add(data_bytes);
        if end > file_len {
            continue;
        }
        if end == file_len || trailer_flag_at(f, end, file_len) {
            data_offset = Some(off);
            break;
        }
    }
    let data_offset = match data_offset {
        Some(o) => o,
        // a file cut inside the data: the data starts at the first candidate
        None => start + candidates[0] as u64,
    };
    let _ = (w, h);
    Ok(Block {
        role,
        start,
        version,
        acquired_unix,
        temperature_c,
        exposure_s,
        accumulation_cycle_s,
        accumulated_cycle_s,
        accumulations,
        kinetic_cycle_s,
        readout_s,
        line3_rest,
        detector,
        detector_size: (u64::try_from(w).unwrap_or(0), u64::try_from(h).unwrap_or(0)),
        original_name,
        user_text,
        calibration,
        frame_calibrations,
        settings_lines,
        frame_axis,
        data_type,
        image_axis,
        area,
        frames,
        frame_pixels,
        sub_images,
        frame_times_us,
        data_offset,
    })
}

/// True when a `0`/`1` flag line starts at `at`.
fn trailer_flag_at(f: &SourceFile, at: u64, file_len: u64) -> bool {
    let mut b = [0u8; 2];
    if at + 2 > file_len || f.read_exact_at(at, &mut b).is_err() {
        return false;
    }
    (b[0] == b'0' || b[0] == b'1') && b[1] == b'\n'
}

/// Parse a SIF file.
pub fn parse(f: &SourceFile, file_len: u64) -> Result<SifLayout> {
    let mut head = vec![0u8; usize::try_from(file_len.min(64)).unwrap_or(0)];
    f.read_exact_at(0, &mut head)
        .map_err(|e| Error::corrupt_at(FORMAT_ID, 0, format!("cannot read the header: {e}")))?;
    if !looks_like_sif(&head) {
        return Err(Error::corrupt(
            FORMAT_ID,
            "not an Andor SIF file: the signature `Andor Technology Multi-Channel File` is missing",
        ));
    }
    // the second line: a value and whether the signal is present
    let second = &head[SIF_MAGIC.len()..];
    let nl = second
        .iter()
        .position(|c| *c == b'\n')
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, 36, "the second line has no end"))?;
    let mut pos = (SIF_MAGIC.len() + nl + 1) as u64;
    let mut blocks = vec![parse_block(f, pos, file_len, 0)?];
    let mut problems = Vec::new();
    pos = blocks[0].data_end();
    let mut trailing_bytes = 0;
    let mut xml = None;
    if pos > file_len {
        problems.push(format!(
            "the signal data ends at {pos}, past the end of the file ({file_len} bytes): the file is cut short"
        ));
        return Ok(SifLayout {
            file_len,
            blocks,
            xml,
            trailing_bytes,
            problems,
        });
    }
    // four flags, each followed by its block when 1
    for (role, block_name) in BLOCK_NAMES.iter().enumerate().skip(1) {
        if !trailer_flag_at(f, pos, file_len) {
            if pos < file_len {
                problems.push(format!("no flag for the {block_name} block at {pos}"));
            }
            break;
        }
        let mut b = [0u8; 1];
        f.read_exact_at(pos, &mut b)
            .map_err(|e| Error::corrupt_at(FORMAT_ID, pos, e.to_string()))?;
        pos += 2;
        if b[0] == b'1' {
            match parse_block(f, pos, file_len, role) {
                Ok(bl) => {
                    pos = bl.data_end();
                    blocks.push(bl);
                    if pos > file_len {
                        problems.push(format!(
                            "the {block_name} data runs past the end of the file"
                        ));
                        break;
                    }
                }
                Err(e) => {
                    problems.push(format!("the {block_name} block cannot be read: {e}"));
                    break;
                }
            }
        }
    }
    if pos < file_len {
        let n = (file_len - pos).min(MAX_XML);
        let mut b = vec![0u8; usize::try_from(n).unwrap_or(0)];
        f.read_exact_at(pos, &mut b)
            .map_err(|e| Error::corrupt_at(FORMAT_ID, pos, e.to_string()))?;
        let t = b.trim_ascii_start();
        if t.starts_with(b"<?xml") || t.starts_with(b"<") {
            xml = Some(
                String::from_utf8_lossy(t)
                    .trim_end_matches('\0')
                    .to_string(),
            );
        } else {
            trailing_bytes = file_len - pos;
        }
    }
    Ok(SifLayout {
        file_len,
        blocks,
        xml,
        trailing_bytes,
        problems,
    })
}
