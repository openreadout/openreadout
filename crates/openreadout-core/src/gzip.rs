//! Random access into gzip-compressed files (`.mzML.gz`, `.mzXML.gz`, ...).
//!
//! gzip (RFC 1952) wraps DEFLATE (RFC 1951), which can only be decoded from the start. Readers
//! of large XML files need random access (an mzML's offset index points into the *decompressed*
//! bytes), so [`GzipSource`] decompresses the file once when it is opened and records
//! **checkpoints** on the way: at DEFLATE block boundaries roughly every megabyte of output it
//! keeps the decoder's bit position and the last 32 KiB of output (the most a back-reference can
//! reach). A later read starts from the nearest checkpoint at or before the wanted offset
//! instead of from the beginning, and a read that follows the previous one continues the live
//! decoder, so reading a file front to back costs one decompression. The same technique is
//! used by zlib's `zran.c` example and by indexed-gzip tools.
//!
//! Costs: opening decompresses the whole file once (hundreds of MB/s); memory is at most
//! [`MAX_CHECKPOINTS`] × 32 KiB (the spacing grows with the file to stay under it) plus a
//! 1 MiB buffer for each of up to four live decoders. Every member's CRC-32 and length are
//! verified during that pass.
//! Concatenated members (`cat a.gz b.gz`, bgzip) are one stream, as `gzip -d` treats them.
//! A truncated or damaged file keeps the bytes that decoded cleanly readable and says why in
//! [`GzipSource::problem`].

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::inflate_flags::{
    TINFL_FLAG_HAS_MORE_INPUT, TINFL_FLAG_STOP_ON_BLOCK_BOUNDARY,
    TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
};
use miniz_oxide::inflate::core::{BlockBoundaryState, DecompressorOxide, decompress};

use crate::error::{Error, Result};
use crate::source::ByteSource;

/// Most checkpoints kept; beyond it every other one is dropped and the spacing doubles.
pub const MAX_CHECKPOINTS: usize = 1024;
/// Output between two checkpoints to begin with (bytes).
const FIRST_SPACING: u64 = 1 << 20;
/// Largest back-reference distance of DEFLATE.
const WINDOW: usize = 32 * 1024;
/// Wrapping output buffer (a power of two, larger than the window): recent output stays in it,
/// so a read slightly behind the previous one is served without decoding again.
const WRAP: usize = 1 << 20;
/// Compressed bytes read at a time.
const IN_CHUNK: usize = 256 * 1024;
/// Live decoders kept (least recently used is replaced).
const LIVE_DECODERS: usize = 4;

/// Live decoders, each with the tick of its last use.
#[derive(Default)]
struct Pool {
    tick: u64,
    streams: Vec<(u64, Stream)>,
}

/// True when `head` starts like a gzip member (magic `1F 8B`, method 8 = deflate).
pub fn is_gzip(head: &[u8]) -> bool {
    head.len() >= 3 && head[0] == 0x1f && head[1] == 0x8b && head[2] == 8
}

/// The first up to `max_out` decompressed bytes of a gzip file whose first bytes are `head`
/// (for format detection from a sniffed head). `None` when `head` is not gzip or its header
/// is incomplete; a truncated or damaged stream yields whatever decoded before the problem.
pub fn peek(head: &[u8], max_out: usize) -> Option<Vec<u8>> {
    let hdr = parse_header(head).ok()??;
    let data = head.get(hdr.len..)?;
    let mut r = DecompressorOxide::new();
    let mut out = vec![0u8; max_out];
    let (_, _, n) = decompress(
        &mut r,
        data,
        &mut out,
        0,
        TINFL_FLAG_HAS_MORE_INPUT | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
    );
    out.truncate(n);
    Some(out)
}

/// A parsed gzip member header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GzipHeader {
    /// Header length in bytes (where the DEFLATE data starts).
    pub len: usize,
    /// `FNAME`: the original file name, when recorded.
    pub name: Option<String>,
    /// `MTIME`: modification time of the original (Unix seconds; 0 = not recorded).
    pub mtime: u32,
    /// `FCOMMENT`, when recorded.
    pub comment: Option<String>,
}

/// Parse a member header at the start of `b`. `Ok(None)`: `b` ends inside the header.
fn parse_header(b: &[u8]) -> std::result::Result<Option<GzipHeader>, String> {
    if b.len() < 10 {
        return Ok(None);
    }
    if b[0] != 0x1f || b[1] != 0x8b {
        return Err("not a gzip member (no 1F 8B signature)".into());
    }
    if b[2] != 8 {
        return Err(format!(
            "gzip compression method {} (only 8, deflate, exists)",
            b[2]
        ));
    }
    let flg = b[3];
    if flg & 0xE0 != 0 {
        return Err(format!("gzip header flags {flg:#04x} set reserved bits"));
    }
    let mtime = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let mut at = 10usize;
    if flg & 0x04 != 0 {
        let Some(x) = b.get(at..at + 2) else {
            return Ok(None);
        };
        at += 2 + usize::from(u16::from_le_bytes([x[0], x[1]]));
    }
    let zstring = |at: &mut usize| -> Option<String> {
        let rest = b.get(*at..)?;
        let end = rest.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        *at += end + 1;
        Some(s)
    };
    let mut name = None;
    if flg & 0x08 != 0 {
        match zstring(&mut at) {
            Some(s) => name = Some(s),
            None => return Ok(None),
        }
    }
    let mut comment = None;
    if flg & 0x10 != 0 {
        match zstring(&mut at) {
            Some(s) => comment = Some(s),
            None => return Ok(None),
        }
    }
    if flg & 0x02 != 0 {
        at += 2;
    }
    if at > b.len() {
        return Ok(None);
    }
    Ok(Some(GzipHeader {
        len: at,
        name,
        mtime,
        comment,
    }))
}

/// Where decoding can restart.
#[derive(Clone)]
struct Checkpoint {
    /// Decompressed offset.
    out: u64,
    /// Compressed offset of the next byte to feed the decoder.
    input: u64,
    /// Decoder state between blocks; `None` at the start of a member (fresh decoder).
    state: Option<BlockBoundaryState>,
    /// Up to 32 KiB of output before `out` (empty at the start of a member).
    window: Arc<[u8]>,
}

impl std::fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoint")
            .field("out", &self.out)
            .field("input", &self.input)
            .field("member_start", &self.state.is_none())
            .finish_non_exhaustive()
    }
}

/// What one decoding step produced.
enum Step {
    /// `n` bytes of output were written at the buffer position before the step.
    Output { at: usize, n: usize },
    /// A DEFLATE block ended (only when asked to stop there).
    Block,
    /// A new member started (its header has been read).
    Member,
    /// End of the data (clean, truncated or damaged: see `problem`).
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Deflate,
    Trailer,
    Header,
    End,
}

/// A running decoder.
struct Stream {
    r: Box<DecompressorOxide>,
    buf: Box<[u8]>,
    /// Write position in `buf`.
    pos: usize,
    /// Total output so far (decompressed offset of `buf[pos]`).
    out: u64,
    /// Buffered compressed bytes; `inbuf[ip]` is at compressed offset `next_in`.
    inbuf: Vec<u8>,
    ip: usize,
    next_in: u64,
    /// No more compressed bytes can be read.
    eof: bool,
    phase: Phase,
    /// CRC-32 and length of the current member's output (verified when `verify`).
    crc: crc32fast::Hasher,
    member_out: u64,
    verify: bool,
    members: u32,
    /// Output bytes available in `buf` before `pos` (at most `WRAP`).
    history: usize,
    problem: Option<String>,
    header: Option<GzipHeader>,
    /// A block ended in the step that returned output; report it next.
    pending_block: bool,
}

impl Stream {
    fn at_member_start(input: u64, out: u64, verify: bool) -> Self {
        Stream {
            r: Box::default(),
            buf: vec![0u8; WRAP].into_boxed_slice(),
            pos: 0,
            out,
            inbuf: Vec::new(),
            ip: 0,
            next_in: input,
            eof: false,
            phase: Phase::Header,
            crc: crc32fast::Hasher::new(),
            member_out: 0,
            verify,
            members: 0,
            history: 0,
            problem: None,
            header: None,
            pending_block: false,
        }
    }

    fn from_checkpoint(c: &Checkpoint) -> Self {
        let mut s = Stream::at_member_start(c.input, c.out, false);
        match &c.state {
            None => s.phase = Phase::Deflate,
            Some(st) => {
                s.r = Box::new(DecompressorOxide::from_block_boundary_state(st));
                s.phase = Phase::Deflate;
            }
        }
        let w = c.window.len().min(WINDOW);
        s.buf[WINDOW - w..WINDOW].copy_from_slice(&c.window[c.window.len() - w..]);
        s.pos = WINDOW;
        s.history = w;
        s
    }

    /// Up to the last `n` output bytes (for a checkpoint window).
    fn tail(&self, n: usize) -> Vec<u8> {
        let n = n.min(self.history);
        let mut v = Vec::with_capacity(n);
        let start = (self.pos + WRAP - n) & (WRAP - 1);
        if start + n <= WRAP {
            v.extend_from_slice(&self.buf[start..start + n]);
        } else {
            v.extend_from_slice(&self.buf[start..]);
            v.extend_from_slice(&self.buf[..n - (WRAP - start)]);
        }
        v
    }

    /// Make at least one unread compressed byte available (unless at the end).
    fn fill(&mut self, src: &dyn ByteSource) -> io::Result<()> {
        if self.ip < self.inbuf.len() || self.eof {
            return Ok(());
        }
        self.inbuf.resize(IN_CHUNK, 0);
        let mut got = 0;
        while got == 0 {
            match src.read_at(self.next_in, &mut self.inbuf) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => got = n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        self.inbuf.truncate(got);
        self.ip = 0;
        Ok(())
    }

    /// Take exactly `n` compressed bytes (fewer at the end of the file).
    fn take(&mut self, src: &dyn ByteSource, n: usize) -> io::Result<Vec<u8>> {
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            self.fill(src)?;
            if self.ip >= self.inbuf.len() {
                break;
            }
            let k = (n - v.len()).min(self.inbuf.len() - self.ip);
            v.extend_from_slice(&self.inbuf[self.ip..self.ip + k]);
            self.ip += k;
            self.next_in += k as u64;
        }
        Ok(v)
    }

    fn fail(&mut self, why: String) -> Step {
        self.problem = Some(why);
        self.phase = Phase::End;
        Step::End
    }

    /// Decode a little further.
    fn step(&mut self, src: &dyn ByteSource, stop_on_block: bool) -> io::Result<Step> {
        if std::mem::take(&mut self.pending_block) {
            return Ok(Step::Block);
        }
        loop {
            match self.phase {
                Phase::End => return Ok(Step::End),
                Phase::Header => return self.read_header(src),
                Phase::Trailer => return self.read_trailer(src),
                Phase::Deflate => {}
            }
            self.fill(src)?;
            let mut flags = 0;
            if !self.eof {
                flags |= TINFL_FLAG_HAS_MORE_INPUT;
            }
            if stop_on_block {
                flags |= TINFL_FLAG_STOP_ON_BLOCK_BOUNDARY;
            }
            let at = self.pos;
            let avail = self.inbuf.len() - self.ip;
            let (status, nin, nout) = decompress(
                &mut self.r,
                &self.inbuf[self.ip..],
                &mut self.buf,
                at,
                flags,
            );
            self.ip += nin;
            self.next_in += nin as u64;
            if nout > 0 {
                if self.verify {
                    self.crc.update(&self.buf[at..at + nout]);
                }
                self.member_out += nout as u64;
                self.out += nout as u64;
                self.pos = (at + nout) & (WRAP - 1);
                self.history = (self.history + nout).min(WRAP);
            }
            let truncated = || {
                format!(
                    "the file ends inside compressed data (truncated after {} decompressed bytes)",
                    self.out
                )
            };
            match status {
                TINFLStatus::Done => self.phase = Phase::Trailer,
                TINFLStatus::BlockBoundary => {
                    if nout == 0 {
                        return Ok(Step::Block);
                    }
                    self.pending_block = true;
                }
                TINFLStatus::HasMoreOutput => {}
                TINFLStatus::NeedsMoreInput if !self.eof && (nin > 0 || avail == 0) => {}
                TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => {
                    let why = truncated();
                    return Ok(self.fail(why));
                }
                other => {
                    let why = format!(
                        "damaged deflate data near compressed byte {} ({other:?}); the first {} decompressed bytes are readable",
                        self.next_in, self.out
                    );
                    return Ok(self.fail(why));
                }
            }
            if nout > 0 {
                return Ok(Step::Output { at, n: nout });
            }
        }
    }

    fn read_trailer(&mut self, src: &dyn ByteSource) -> io::Result<Step> {
        let t = self.take(src, 8)?;
        if t.len() < 8 {
            return Ok(self.fail(format!(
                "the file ends inside the gzip trailer of member {} (truncated)",
                self.members.max(1)
            )));
        }
        if self.verify {
            let want = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
            let size = u32::from_le_bytes([t[4], t[5], t[6], t[7]]);
            let got = std::mem::replace(&mut self.crc, crc32fast::Hasher::new()).finalize();
            if want != got {
                return Ok(self.fail(format!(
                    "CRC-32 mismatch in gzip member {} (recorded {want:08x}, data {got:08x}): the data is damaged",
                    self.members.max(1)
                )));
            }
            if u64::from(size) != self.member_out & 0xFFFF_FFFF {
                return Ok(self.fail(format!(
                    "gzip member {} length mismatch (recorded {size} mod 2^32, decoded {})",
                    self.members.max(1),
                    self.member_out
                )));
            }
        }
        self.phase = Phase::Header;
        self.read_header(src)
    }

    fn read_header(&mut self, src: &dyn ByteSource) -> io::Result<Step> {
        // Look at up to 64 KiB without consuming; headers are short unless FEXTRA is large.
        self.fill(src)?;
        if self.ip >= self.inbuf.len() {
            if self.members == 0 {
                return Ok(self.fail("the file is empty".into()));
            }
            self.phase = Phase::End;
            return Ok(Step::End);
        }
        let mut peek = self.inbuf[self.ip..].to_vec();
        if peek.len() < 64 * 1024 && !self.eof {
            let mut more = vec![0u8; 64 * 1024 - peek.len()];
            let n = src.read_at(self.next_in + peek.len() as u64, &mut more)?;
            peek.extend_from_slice(&more[..n]);
        }
        if self.members > 0 && peek.iter().all(|&b| b == 0) {
            // zero padding after the last member (tape blocks, some writers)
            self.phase = Phase::End;
            return Ok(Step::End);
        }
        match parse_header(&peek) {
            Ok(Some(h)) => {
                let n = h.len;
                self.take(src, n)?;
                if self.header.is_none() {
                    self.header = Some(h);
                }
                self.members += 1;
                *self.r = DecompressorOxide::new();
                self.crc = crc32fast::Hasher::new();
                self.member_out = 0;
                self.phase = Phase::Deflate;
                Ok(Step::Member)
            }
            Ok(None) => Ok(self.fail("the file ends inside a gzip header (truncated)".into())),
            Err(e) if self.members > 0 => Ok(self.fail(format!(
                "{} bytes after gzip member {} are not gzip data and were ignored ({e})",
                peek.len(),
                self.members
            ))),
            Err(e) => Ok(self.fail(e)),
        }
    }
}

/// Summary of a gzip file, from the pass made at open.
#[derive(Debug, Clone, Default)]
pub struct GzipSummary {
    /// Number of gzip members (usually 1).
    pub members: u32,
    /// Compressed size in bytes.
    pub compressed_len: u64,
    /// Decompressed size in bytes (of the part that decoded cleanly).
    pub decompressed_len: u64,
    /// Restart points kept.
    pub checkpoints: usize,
    /// The first member's header (original name, time stamp, comment).
    pub header: GzipHeader,
    /// Why the data ends early or is suspect, when it does.
    pub problem: Option<String>,
}

/// The decompressed bytes of a gzip file, with random access (see the module docs).
pub struct GzipSource {
    inner: Arc<dyn ByteSource>,
    name: String,
    points: Vec<Checkpoint>,
    summary: GzipSummary,
    live: Mutex<Pool>,
}

impl std::fmt::Debug for GzipSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GzipSource")
            .field("name", &self.name)
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

impl GzipSource {
    /// Decompress `inner` once, verifying every member, and keep restart points. `name` names
    /// the decompressed content in messages. A file that is not gzip at all is an error
    /// (corrupt, `format` names the caller); a truncated or damaged one opens with
    /// [`problem`](GzipSource::problem) set and its clean prefix readable.
    pub fn open(inner: Arc<dyn ByteSource>, name: String, format: &'static str) -> Result<Self> {
        let io_err = |e: io::Error| Error::io(std::path::Path::new(&inner.name()), e);
        let compressed_len = inner.size().map_err(io_err)?;
        let mut s = Stream::at_member_start(0, 0, true);
        let mut points: Vec<Checkpoint> = Vec::new();
        let mut spacing = FIRST_SPACING;
        loop {
            match s.step(inner.as_ref(), true).map_err(io_err)? {
                Step::Member => points.push(Checkpoint {
                    out: s.out,
                    input: s.next_in,
                    state: None,
                    window: Arc::from(Vec::new()),
                }),
                Step::Block => {
                    let last = points.last().map_or(0, |p| p.out);
                    if s.out.saturating_sub(last) >= spacing
                        && let Some(state) = s.r.block_boundary_state()
                    {
                        points.push(Checkpoint {
                            out: s.out,
                            input: s.next_in,
                            state: Some(state),
                            window: Arc::from(s.tail(WINDOW)),
                        });
                        if points.len() > MAX_CHECKPOINTS {
                            // keep member starts and every other block checkpoint
                            let mut keep = false;
                            points.retain(|p| {
                                keep = !keep;
                                keep || p.state.is_none()
                            });
                            spacing = spacing.saturating_mul(2);
                        }
                    }
                }
                Step::Output { .. } => {}
                Step::End => break,
            }
        }
        if s.members == 0 {
            return Err(Error::corrupt(
                format,
                format!(
                    "not a readable gzip file: {}",
                    s.problem.unwrap_or_else(|| "no gzip member".into())
                ),
            ));
        }
        let summary = GzipSummary {
            members: s.members,
            compressed_len,
            decompressed_len: s.out,
            checkpoints: points.len(),
            header: s.header.clone().unwrap_or_default(),
            problem: s.problem.clone(),
        };
        Ok(GzipSource {
            inner,
            name,
            points,
            summary,
            live: Mutex::new(Pool::default()),
        })
    }

    /// What the pass at open found.
    pub fn summary(&self) -> &GzipSummary {
        &self.summary
    }

    /// Why the decompressed data ends early, when it does.
    pub fn problem(&self) -> Option<&str> {
        self.summary.problem.as_deref()
    }

    fn read_into(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.summary.decompressed_len;
        if offset >= len || buf.is_empty() {
            return Ok(0);
        }
        let want = buf
            .len()
            .min(usize::try_from(len - offset).unwrap_or(usize::MAX));
        let end = offset + want as u64;
        let mut pool = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        pool.tick += 1;
        let tick = pool.tick;
        // Continue a live decoder when the wanted bytes are still in its buffer, or lie ahead of
        // it but before the next restart point; otherwise restart at the nearest checkpoint.
        // Several decoders are kept because readers alternate between places (an XML reader
        // re-reads the document head before each element).
        let cp = self.points.partition_point(|p| p.out <= offset);
        let cp = &self.points[cp.saturating_sub(1)];
        let best = pool
            .streams
            .iter()
            .enumerate()
            .filter(|(_, (_, s))| {
                let behind = s.out.saturating_sub(s.history as u64);
                // Continuing costs `offset - s.out`, restarting `offset - cp.out` plus the rest
                // of the chunk the old decoder already produced (up to one buffer).
                offset >= behind && s.out + WRAP as u64 >= cp.out
            })
            .min_by_key(|(_, (_, s))| offset.saturating_sub(s.out))
            .map(|(i, _)| i);
        let i = if let Some(i) = best {
            i
        } else {
            let fresh = Stream::from_checkpoint(cp);
            if pool.streams.len() < LIVE_DECODERS {
                pool.streams.push((tick, fresh));
                pool.streams.len() - 1
            } else {
                let lru = pool
                    .streams
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, (t, _))| *t)
                    .map_or(0, |(i, _)| i);
                pool.streams[lru] = (tick, fresh);
                lru
            }
        };
        let Some((used, s)) = pool.streams.get_mut(i) else {
            return Ok(0);
        };
        *used = tick;
        let mut filled = 0usize;
        // already decoded bytes in the buffer
        if offset < s.out {
            let back = (s.out - offset) as usize;
            let n = back.min(want);
            let start = (s.pos + WRAP - back) & (WRAP - 1);
            copy_wrapped(&s.buf, start, &mut buf[..n]);
            filled = n;
        }
        while filled < want {
            match s.step(self.inner.as_ref(), false)? {
                Step::Output { at, n } => {
                    let first = s.out - n as u64; // decompressed offset of buf[at]
                    let lo = (offset + filled as u64).max(first);
                    let hi = end.min(s.out);
                    if lo < hi {
                        let from = at + (lo - first) as usize;
                        let k = (hi - lo) as usize;
                        buf[filled..filled + k].copy_from_slice(&s.buf[from..from + k]);
                        filled += k;
                    }
                }
                Step::Block | Step::Member => {}
                Step::End => break,
            }
        }
        Ok(filled)
    }
}

fn copy_wrapped(ring: &[u8], start: usize, out: &mut [u8]) {
    let n = out.len();
    if start + n <= ring.len() {
        out.copy_from_slice(&ring[start..start + n]);
    } else {
        let a = ring.len() - start;
        out[..a].copy_from_slice(&ring[start..]);
        out[a..].copy_from_slice(&ring[..n - a]);
    }
}

impl ByteSource for GzipSource {
    fn size(&self) -> io::Result<u64> {
        Ok(self.summary.decompressed_len)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.read_into(offset, buf)
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

/// `name` without a trailing `.gz` (any case): the name of the decompressed content.
pub fn strip_gz(name: &str) -> &str {
    let n = name.len();
    if n > 3 && name.is_char_boundary(n - 3) && name[n - 3..].eq_ignore_ascii_case(".gz") {
        &name[..n - 3]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemSource;
    use std::io::Write;

    fn gz(data: &[u8], level: u32) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn sample(n: usize) -> Vec<u8> {
        // compressible but not trivial: pseudo-random words
        let mut x = 0x1234_5678u32;
        let words = [
            &b"<spectrum "[..],
            b"id=\"scan=",
            b"\"/>\n",
            b"cvParam ",
            b"value=\"",
        ];
        let mut v = Vec::with_capacity(n + 16);
        while v.len() < n {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            v.extend_from_slice(words[(x % 5) as usize]);
            v.extend_from_slice((x % 100_000).to_string().as_bytes());
        }
        v.truncate(n);
        v
    }

    fn open(bytes: Vec<u8>) -> GzipSource {
        GzipSource::open(Arc::new(MemSource::new("t.gz", bytes)), "t".into(), "test").unwrap()
    }

    #[test]
    fn random_and_sequential_reads_match() {
        let data = sample(9 << 20);
        let src = open(gz(&data, 6));
        assert!(src.problem().is_none());
        assert_eq!(src.size().unwrap(), data.len() as u64);
        assert!(src.summary().checkpoints > 4, "{:?}", src.summary());
        // random reads, backwards and forwards
        let mut x = 7u64;
        for _ in 0..200 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let off = x % data.len() as u64;
            let len = ((x >> 40) % 70_000) as usize;
            let mut b = vec![0u8; len];
            let n = src.read_at(off, &mut b).unwrap();
            let want = len.min(data.len() - off as usize);
            assert_eq!(n, want);
            assert_eq!(&b[..n], &data[off as usize..off as usize + n]);
        }
        // front-to-back in small pieces, with small backward steps
        let mut off = 0usize;
        while off < data.len() {
            let mut b = vec![0u8; 5000];
            let n = src.read_at(off as u64, &mut b).unwrap();
            assert_eq!(&b[..n], &data[off..off + n]);
            off += n.saturating_sub(100).max(1);
        }
    }

    #[test]
    fn stored_and_multi_member_streams() {
        let a = sample(3 << 20);
        let b = sample(1 << 20);
        let mut bytes = gz(&a, 0); // stored blocks
        bytes.extend(gz(&b, 9));
        bytes.extend([0u8; 16]); // zero padding
        let src = open(bytes);
        assert_eq!(src.summary().members, 2);
        assert!(src.problem().is_none(), "{:?}", src.problem());
        let mut all = a.clone();
        all.extend_from_slice(&b);
        let mut got = vec![0u8; all.len()];
        src.read_exact_at(0, &mut got).unwrap();
        assert_eq!(got, all);
        let mut tail = vec![0u8; 1000];
        src.read_exact_at(all.len() as u64 - 1000, &mut tail)
            .unwrap();
        assert_eq!(tail, all[all.len() - 1000..]);
    }

    #[test]
    fn truncated_and_damaged_files_keep_their_prefix() {
        let data = sample(4 << 20);
        let full = gz(&data, 6);
        let cut = full[..full.len() / 2].to_vec();
        let src = open(cut);
        let p = src.problem().unwrap();
        assert!(p.contains("truncated"), "{p}");
        let n = src.size().unwrap() as usize;
        assert!(n > 0 && n < data.len());
        let mut got = vec![0u8; n];
        src.read_exact_at(0, &mut got).unwrap();
        assert_eq!(got, data[..n]);

        let mut bad = full.clone();
        let l = bad.len();
        bad[l - 6] ^= 0xFF; // CRC byte
        let src = open(bad);
        assert!(
            src.problem().unwrap().contains("CRC"),
            "{:?}",
            src.problem()
        );

        assert!(
            GzipSource::open(
                Arc::new(MemSource::new("x", b"plain text".to_vec())),
                "x".into(),
                "t"
            )
            .is_err()
        );
    }

    #[test]
    fn headers_peek_and_names() {
        let mut e = flate2::GzBuilder::new()
            .filename("run.mzML")
            .comment("c")
            .extra(vec![1, 2, 3])
            .write(Vec::new(), flate2::Compression::default());
        e.write_all(b"<?xml version=\"1.0\"?><mzML>").unwrap();
        let bytes = e.finish().unwrap();
        assert!(is_gzip(&bytes));
        let h = parse_header(&bytes).unwrap().unwrap();
        assert_eq!(h.name.as_deref(), Some("run.mzML"));
        assert_eq!(h.comment.as_deref(), Some("c"));
        assert_eq!(peek(&bytes, 100).unwrap(), b"<?xml version=\"1.0\"?><mzML>");
        assert_eq!(peek(&bytes[..5], 100), None);
        assert_eq!(strip_gz("a.mzML.GZ"), "a.mzML");
        assert_eq!(strip_gz("a.mzML"), "a.mzML");
        for cut in 0..bytes.len() {
            let _ = parse_header(&bytes[..cut]);
            let _ = peek(&bytes[..cut], 64);
        }
    }
}
