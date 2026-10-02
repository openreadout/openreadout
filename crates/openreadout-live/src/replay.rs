//! Acquisition replay: rewrite a finished file into a new file (or directory) step by step,
//! in the order acquisition software plausibly writes it, so that reading growing files and
//! `watch` can be tested without an instrument. After the last step the output is
//! byte-identical to the source.
//!
//! Each [`Pattern`] is marked [`PatternStatus::Documented`] or [`PatternStatus::Assumed`] in
//! [`Pattern::status`] and `book/src/guides/lab-shares.md`: none has been checked against a running instrument.
//!
//! | pattern | steps | end of acquisition |
//! | --- | --- | --- |
//! | `ome-tiff` | header, then per page: IFD (next pointer 0) and its pixel data, then the previous IFD's next pointer | OME-XML appended and the first page's description patched to point at it |
//! | `ome-zarr` | metadata documents first, then level-0 chunk files one by one (written to a temporary name and renamed) | downsampled levels' chunks |
//! | `nd2` | chunks appended in file order (metadata chunks, frames) | closing metadata chunks and the chunk map |
//! | `czi` | file header with directory, metadata and attachment positions 0; directory and metadata segments reserved (header only, zero-filled); subblocks appended | reserved segments filled, header positions patched |
//! | `append` | the file appended in fixed-size blocks | — |
//!
//! The source is only read; the destination must not exist.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use openreadout_core::bytes::{Endian, find, le_u32, le_u64};
use openreadout_core::{Error, Result};

/// A write pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pattern {
    /// OME-TIFF (classic TIFF or BigTIFF): IFD by IFD, OME-XML finalized at the end.
    OmeTiff,
    /// OME-Zarr directory store: metadata first, chunk files one by one.
    OmeZarr,
    /// Nikon ND2: chunks appended, chunk map last.
    Nd2,
    /// Zeiss CZI: subblocks appended, directory and metadata filled in at the end.
    Czi,
    /// Plain append in fixed-size blocks (Thermo RAW and any format whose pattern is unknown).
    Append,
}

/// How much is known about a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatternStatus {
    /// Supported by the structure of real files (what comes last in the file) and public
    /// descriptions; the exact order of writes is still not observed on an instrument.
    Documented,
    /// A plausible order chosen by us; not verified.
    Assumed,
}

impl Pattern {
    /// Parse a pattern name (`ome-tiff`, `ome-zarr`, `nd2`, `czi`, `append`).
    pub fn parse(s: &str) -> Option<Pattern> {
        Some(match s.to_ascii_lowercase().as_str() {
            "ome-tiff" | "ometiff" | "tiff" => Pattern::OmeTiff,
            "ome-zarr" | "omezarr" | "zarr" => Pattern::OmeZarr,
            "nd2" => Pattern::Nd2,
            "czi" => Pattern::Czi,
            "append" | "thermo-raw" | "raw" => Pattern::Append,
            _ => return None,
        })
    }

    /// The pattern for a path, from its extension (`None` when unknown).
    pub fn for_path(p: &Path) -> Option<Pattern> {
        let n = p.file_name()?.to_string_lossy().to_ascii_lowercase();
        if p.is_dir() || Path::new(&n).extension().is_some_and(|e| e == "zarr") {
            return Some(Pattern::OmeZarr);
        }
        let ext = n.rsplit('.').next().unwrap_or("");
        Some(match ext {
            "tif" | "tiff" | "btf" | "tf2" | "tf8" => Pattern::OmeTiff,
            "nd2" => Pattern::Nd2,
            "czi" => Pattern::Czi,
            _ => Pattern::Append,
        })
    }

    /// Name used on the command line and in docs.
    pub fn name(self) -> &'static str {
        match self {
            Pattern::OmeTiff => "ome-tiff",
            Pattern::OmeZarr => "ome-zarr",
            Pattern::Nd2 => "nd2",
            Pattern::Czi => "czi",
            Pattern::Append => "append",
        }
    }

    /// Whether the order is documented or assumed (see `book/src/guides/lab-shares.md`).
    pub fn status(self) -> PatternStatus {
        match self {
            // The ND2 chunk map is at the very end and frames sit between two metadata
            // blocks; the OME-XML of the corpus OME-TIFFs is stored after the last plane.
            Pattern::Nd2 | Pattern::OmeTiff => PatternStatus::Documented,
            Pattern::OmeZarr | Pattern::Czi | Pattern::Append => PatternStatus::Assumed,
        }
    }
}

/// Bytes of one write.
#[derive(Debug, Clone)]
enum Data {
    /// `len` bytes of the source at `offset`.
    Src(u64, u64),
    /// Literal bytes (placeholders).
    Bytes(Vec<u8>),
    /// Zero bytes (reserved space).
    Zeros(u64),
}

#[derive(Debug, Clone)]
enum Op {
    /// Write into the destination file (or `rel` inside the destination directory) at `offset`.
    At {
        rel: Option<PathBuf>,
        offset: u64,
        data: Data,
    },
    /// Copy a whole file of a directory source to `rel`, through a temporary name and a rename.
    File { rel: PathBuf },
}

/// One step of a replay: the writes that make one more unit (plane, chunk, scan block)
/// appear, or part of one.
#[derive(Debug, Clone)]
pub struct Step {
    ops: Vec<Op>,
    /// What the step completes: `header`, `plane`, `partial` (half a unit), `chunk`,
    /// `metadata`, `finalize`, `block`.
    pub kind: &'static str,
    /// Index of the unit completed by this step (planes, frame chunks, chunk files), if any.
    pub unit: Option<u64>,
}

/// A planned replay.
#[derive(Debug)]
pub struct Plan {
    src: PathBuf,
    /// The pattern used.
    pub pattern: Pattern,
    /// Steps in order.
    pub steps: Vec<Step>,
    dir: bool,
}

impl Plan {
    /// Number of units (planes, frames, level-0 chunks, blocks) the replay produces.
    pub fn units(&self) -> u64 {
        self.steps.iter().filter(|s| s.unit.is_some()).count() as u64
    }

    /// Index of the step after which `n` units are complete (`None` if there are fewer).
    pub fn step_after_units(&self, n: u64) -> Option<usize> {
        if n == 0 {
            return self
                .steps
                .iter()
                .position(|s| s.unit.is_some())
                .map(|i| i.saturating_sub(1));
        }
        let mut seen = 0;
        for (i, s) in self.steps.iter().enumerate() {
            if s.unit.is_some() {
                seen += 1;
                if seen == n {
                    return Some(i);
                }
            }
        }
        None
    }
}

fn corrupt(msg: impl Into<String>) -> Error {
    Error::corrupt("replay", msg)
}

fn read_all(p: &Path) -> Result<Vec<u8>> {
    std::fs::read(p).map_err(|e| Error::io(p, e))
}

fn tail(b: &[u8], at: u64) -> &[u8] {
    usize::try_from(at)
        .ok()
        .and_then(|i| b.get(i..))
        .unwrap_or(&[])
}

/// Plan a replay of `src` with `pattern`. With `partial`, every unit is written in two steps
/// (half, then the rest), so a reader can be tested on a unit in flight.
pub fn plan(src: &Path, pattern: Pattern, partial: bool) -> Result<Plan> {
    let mut plan = match pattern {
        Pattern::Nd2 => plan_nd2(src)?,
        Pattern::Czi => plan_czi(src)?,
        Pattern::OmeTiff => plan_tiff(src)?,
        Pattern::OmeZarr => plan_zarr(src)?,
        Pattern::Append => plan_append(src, 64 << 10)?,
    };
    if partial {
        plan.steps = split_units(std::mem::take(&mut plan.steps));
    }
    Ok(plan)
}

/// Split each unit step's largest source write in two: a `partial` step, then the rest.
fn split_units(steps: Vec<Step>) -> Vec<Step> {
    let mut out = Vec::with_capacity(steps.len() * 2);
    for s in steps {
        let big = s.ops.iter().enumerate().find_map(|(i, o)| match o {
            Op::At {
                data: Data::Src(_, len),
                ..
            } if *len >= 2 && s.unit.is_some() => Some(i),
            _ => None,
        });
        let Some(bi) = big else {
            out.push(s);
            continue;
        };
        let Op::At {
            rel,
            offset,
            data: Data::Src(so, len),
        } = s.ops[bi].clone()
        else {
            out.push(s);
            continue;
        };
        let half = len / 2;
        let mut first: Vec<Op> = s.ops[..bi].to_vec();
        first.push(Op::At {
            rel: rel.clone(),
            offset,
            data: Data::Src(so, half),
        });
        // Placeholders that land inside the half already written apply to it too (a TIFF
        // page's zeroed next pointer, the first page's empty description).
        for o in &s.ops[bi + 1..] {
            if let Op::At {
                offset: po,
                data: d @ (Data::Bytes(_) | Data::Zeros(_)),
                ..
            } = o
            {
                let n = match d {
                    Data::Bytes(v) => v.len() as u64,
                    Data::Zeros(n) => *n,
                    Data::Src(..) => 0,
                };
                if *po >= offset && po + n <= offset + half {
                    first.push(o.clone());
                }
            }
        }
        let mut rest = vec![Op::At {
            rel,
            offset: offset + half,
            data: Data::Src(so + half, len - half),
        }];
        rest.extend_from_slice(&s.ops[bi + 1..]);
        out.push(Step {
            ops: first,
            kind: "partial",
            unit: None,
        });
        out.push(Step { ops: rest, ..s });
    }
    out
}

fn plan_append(src: &Path, block: u64) -> Result<Plan> {
    let len = std::fs::metadata(src).map_err(|e| Error::io(src, e))?.len();
    let mut steps = Vec::new();
    let mut off = 0u64;
    let mut n = 0u64;
    while off < len {
        let l = block.min(len - off);
        steps.push(Step {
            ops: vec![Op::At {
                rel: None,
                offset: off,
                data: Data::Src(off, l),
            }],
            kind: "block",
            unit: Some(n),
        });
        n += 1;
        off += l;
    }
    Ok(Plan {
        src: src.to_path_buf(),
        pattern: Pattern::Append,
        steps,
        dir: false,
    })
}

const ND2_MAGIC: u32 = 0x0ABE_CEDA;

fn plan_nd2(src: &Path) -> Result<Plan> {
    let b = read_all(src)?;
    let len = b.len() as u64;
    let mut steps = Vec::new();
    let mut off = 0u64;
    let mut frames = 0u64;
    let mut start = 0u64;
    while off + 16 <= len {
        if le_u32(tail(&b, off), 0) != Some(ND2_MAGIC) {
            // Padding between chunks: find the next chunk header.
            let rest = &b[usize::try_from(off).unwrap_or(usize::MAX).min(b.len())..];
            let magic = ND2_MAGIC.to_le_bytes();
            match find(rest, &magic) {
                Some(p) => {
                    off += p as u64;
                    continue;
                }
                None => break,
            }
        }
        let name_len = u64::from(le_u32(tail(&b, off + 4), 0).unwrap_or(0));
        let data_len = le_u64(tail(&b, off + 8), 0).unwrap_or(0);
        let end = off
            .checked_add(16 + name_len)
            .and_then(|v| v.checked_add(data_len))
            .filter(|e| *e <= len)
            .ok_or_else(|| corrupt(format!("ND2 chunk at {off} runs past the end")))?;
        let nb = usize::try_from(name_len).unwrap_or(0).min(64);
        let o = usize::try_from(off).unwrap_or(0) + 16;
        let name = String::from_utf8_lossy(b.get(o..o + nb).unwrap_or(&[])).to_string();
        let is_frame = name.starts_with("ImageDataSeq|");
        steps.push(Step {
            ops: vec![Op::At {
                rel: None,
                offset: start,
                data: Data::Src(start, end - start),
            }],
            kind: if is_frame { "plane" } else { "metadata" },
            unit: is_frame.then_some(frames),
        });
        if is_frame {
            frames += 1;
        }
        start = end;
        off = end;
    }
    if start < len {
        steps.push(Step {
            ops: vec![Op::At {
                rel: None,
                offset: start,
                data: Data::Src(start, len - start),
            }],
            kind: "finalize",
            unit: None,
        });
    }
    if frames == 0 {
        return Err(corrupt("no ND2 frame chunks found"));
    }
    Ok(Plan {
        src: src.to_path_buf(),
        pattern: Pattern::Nd2,
        steps,
        dir: false,
    })
}

fn plan_czi(src: &Path) -> Result<Plan> {
    let b = read_all(src)?;
    let len = b.len() as u64;
    let id = |off: u64| -> String {
        let o = usize::try_from(off).unwrap_or(usize::MAX);
        b.get(o..o.saturating_add(16))
            .map(|s| {
                String::from_utf8_lossy(s)
                    .trim_end_matches('\0')
                    .to_string()
            })
            .unwrap_or_default()
    };
    if id(0) != "ZISRAWFILE" {
        return Err(corrupt("not a CZI (no ZISRAWFILE segment at 0)"));
    }
    let mut segs = Vec::new();
    let mut off = 0u64;
    while off + 32 <= len {
        let alloc = le_u64(tail(&b, off + 16), 0).unwrap_or(0);
        let end = off
            .checked_add(32)
            .and_then(|v| v.checked_add(alloc))
            .filter(|e| *e <= len && alloc > 0)
            .ok_or_else(|| corrupt(format!("CZI segment at {off} runs past the end")))?;
        segs.push((off, end, id(off)));
        off = end;
    }
    let header_end = segs[0].1;
    // Header with the subblock-directory, metadata and attachment-directory positions (64-bit
    // fields at 52, 60 and 72 of the header segment's data) set to 0: not finalized.
    let mut hdr = b[..usize::try_from(header_end).unwrap_or(0)].to_vec();
    for field in [32 + 52usize, 32 + 60, 32 + 72] {
        if let Some(s) = hdr.get_mut(field..field + 8) {
            s.fill(0);
        }
    }
    let mut steps = vec![Step {
        ops: vec![Op::At {
            rel: None,
            offset: 0,
            data: Data::Bytes(hdr),
        }],
        kind: "header",
        unit: None,
    }];
    let reserved = ["ZISRAWDIRECTORY", "ZISRAWATTDIR", "ZISRAWMETADATA"];
    let mut finalize = Vec::new();
    let mut planes = 0u64;
    for (s, e, name) in segs.iter().skip(1) {
        if reserved.contains(&name.as_str()) {
            steps.push(Step {
                ops: vec![
                    Op::At {
                        rel: None,
                        offset: *s,
                        data: Data::Src(*s, 32),
                    },
                    Op::At {
                        rel: None,
                        offset: s + 32,
                        data: Data::Zeros(e - s - 32),
                    },
                ],
                kind: "metadata",
                unit: None,
            });
            finalize.push(Op::At {
                rel: None,
                offset: s + 32,
                data: Data::Src(s + 32, e - s - 32),
            });
        } else {
            let sub = name == "ZISRAWSUBBLOCK";
            steps.push(Step {
                ops: vec![Op::At {
                    rel: None,
                    offset: *s,
                    data: Data::Src(*s, e - s),
                }],
                kind: if sub { "plane" } else { "metadata" },
                unit: sub.then_some(planes),
            });
            if sub {
                planes += 1;
            }
        }
    }
    if off < len {
        finalize.push(Op::At {
            rel: None,
            offset: off,
            data: Data::Src(off, len - off),
        });
    }
    finalize.push(Op::At {
        rel: None,
        offset: 0,
        data: Data::Src(0, header_end),
    });
    steps.push(Step {
        ops: finalize,
        kind: "finalize",
        unit: None,
    });
    Ok(Plan {
        src: src.to_path_buf(),
        pattern: Pattern::Czi,
        steps,
        dir: false,
    })
}

/// One IFD of the main chain: where it is, its next-pointer position, its description entry.
struct TiffIfd {
    offset: u64,
    next_pos: u64,
    ptr_len: u64,
    /// (entry position, entry length, value offset, value length) of an out-of-line
    /// ImageDescription.
    desc: Option<(u64, u64, u64, u64)>,
}

fn tiff_ifds(b: &[u8]) -> Result<(Endian, bool, Vec<TiffIfd>)> {
    let order = match b.get(..2) {
        Some(b"II") => Endian::Little,
        Some(b"MM") => Endian::Big,
        _ => return Err(corrupt("not a TIFF")),
    };
    let big = match order.u16(tail(b, 2), 0) {
        Some(42) => false,
        Some(43) => true,
        _ => return Err(corrupt("not a TIFF")),
    };
    let mut off = if big {
        order.u64(tail(b, 8), 0)
    } else {
        order.u32(tail(b, 4), 0).map(u64::from)
    }
    .unwrap_or(0);
    let mut out = Vec::new();
    while off != 0 {
        if out.len() > 1_000_000 || out.iter().any(|i: &TiffIfd| i.offset == off) {
            return Err(corrupt("IFD chain loops"));
        }
        let (count, entry, ptr, first) = if big {
            (order.u64(tail(b, off), 0), 20u64, 8u64, off + 8)
        } else {
            (order.u16(tail(b, off), 0).map(u64::from), 12, 4, off + 2)
        };
        let count = count.ok_or_else(|| corrupt("IFD past the end"))?;
        let next_pos = first + count * entry;
        let mut desc = None;
        for i in 0..count {
            let e = first + i * entry;
            if order.u16(tail(b, e), 0) != Some(270) {
                continue;
            }
            let (n, v) = if big {
                (order.u64(tail(b, e + 4), 0), order.u64(tail(b, e + 12), 0))
            } else {
                (
                    order.u32(tail(b, e + 4), 0).map(u64::from),
                    order.u32(tail(b, e + 8), 0).map(u64::from),
                )
            };
            if let (Some(n), Some(v)) = (n, v)
                && n > ptr
            {
                desc = Some((e, entry, v, n));
            }
        }
        let next = if big {
            order.u64(tail(b, next_pos), 0)
        } else {
            order.u32(tail(b, next_pos), 0).map(u64::from)
        }
        .ok_or_else(|| corrupt("IFD past the end"))?;
        out.push(TiffIfd {
            offset: off,
            next_pos,
            ptr_len: ptr,
            desc,
        });
        off = next;
    }
    Ok((order, big, out))
}

fn plan_tiff(src: &Path) -> Result<Plan> {
    let b = read_all(src)?;
    let len = b.len() as u64;
    let (order, big, ifds) = tiff_ifds(&b)?;
    let le = order == Endian::Little;
    if ifds.is_empty() || ifds.windows(2).any(|w| w[1].offset <= w[0].offset) {
        return Err(Error::unsupported(
            "replay",
            "a TIFF whose IFDs are not in file order",
            "Use --pattern append.",
        ));
    }
    // OME-XML stored after the last page's IFD is written last (`xml_at`); the first page's
    // description entry holds an empty inline placeholder until then.
    let xml = ifds[0]
        .desc
        .filter(|(_, _, v, n)| *v > ifds[ifds.len() - 1].offset && v + n <= len);
    let xml_at = xml.map_or(len, |(_, _, v, _)| v);
    let mut steps = Vec::new();
    let zero_ptr = |i: &TiffIfd| Op::At {
        rel: None,
        offset: i.next_pos,
        data: Data::Zeros(i.ptr_len),
    };
    for (k, ifd) in ifds.iter().enumerate() {
        let start = if k == 0 { 0 } else { ifd.offset };
        let end = ifds.get(k + 1).map_or(xml_at, |n| n.offset);
        if end < start {
            return Err(corrupt("TIFF pages overlap"));
        }
        let mut ops = vec![
            Op::At {
                rel: None,
                offset: start,
                data: Data::Src(start, end - start),
            },
            zero_ptr(ifd),
        ];
        if k == 0
            && let Some((e, elen, _, _)) = xml
        {
            // ImageDescription, type ASCII, count 1, inline value "\0".
            let mut ent = Vec::with_capacity(20);
            let tag: u16 = 270;
            let ty: u16 = 2;
            ent.extend_from_slice(&if le {
                tag.to_le_bytes()
            } else {
                tag.to_be_bytes()
            });
            ent.extend_from_slice(&if le {
                ty.to_le_bytes()
            } else {
                ty.to_be_bytes()
            });
            if big {
                ent.extend_from_slice(&if le {
                    1u64.to_le_bytes()
                } else {
                    1u64.to_be_bytes()
                });
            } else {
                ent.extend_from_slice(&if le {
                    1u32.to_le_bytes()
                } else {
                    1u32.to_be_bytes()
                });
            }
            ent.resize(usize::try_from(elen).unwrap_or(12), 0);
            ops.push(Op::At {
                rel: None,
                offset: e,
                data: Data::Bytes(ent),
            });
        }
        if k > 0 {
            let p = &ifds[k - 1];
            ops.push(Op::At {
                rel: None,
                offset: p.next_pos,
                data: Data::Src(p.next_pos, p.ptr_len),
            });
        }
        steps.push(Step {
            ops,
            kind: "plane",
            unit: Some(k as u64),
        });
    }
    let mut fin = Vec::new();
    if xml_at < len {
        fin.push(Op::At {
            rel: None,
            offset: xml_at,
            data: Data::Src(xml_at, len - xml_at),
        });
    }
    if let Some((e, elen, _, _)) = xml {
        fin.push(Op::At {
            rel: None,
            offset: e,
            data: Data::Src(e, elen),
        });
    }
    let last = &ifds[ifds.len() - 1];
    fin.push(Op::At {
        rel: None,
        offset: last.next_pos,
        data: Data::Src(last.next_pos, last.ptr_len),
    });
    steps.push(Step {
        ops: fin,
        kind: "finalize",
        unit: None,
    });
    Ok(Plan {
        src: src.to_path_buf(),
        pattern: Pattern::OmeTiff,
        steps,
        dir: false,
    })
}

const ZARR_DOCS: [&str; 5] = ["zarr.json", ".zattrs", ".zgroup", ".zarray", ".zmetadata"];

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(std::result::Result::ok)
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        let p = e.path();
        let ft = e.file_type().map_err(|err| Error::io(&p, err))?;
        if ft.is_dir() {
            walk_files(root, &p, out)?;
        } else if ft.is_file() {
            out.push(p.strip_prefix(root).unwrap_or(&p).to_path_buf());
        }
    }
    Ok(())
}

/// Sort key of a chunk file: its numeric path components (C order of the chunk grid).
fn chunk_key(rel: &Path) -> Vec<u64> {
    rel.to_string_lossy()
        .split(['/', '\\', '.'])
        .filter_map(|c| c.parse::<u64>().ok())
        .collect()
}

fn plan_zarr(src: &Path) -> Result<Plan> {
    if !src.is_dir() {
        return Err(Error::Usage(
            "the ome-zarr pattern replays a directory store".into(),
        ));
    }
    let mut files = Vec::new();
    walk_files(src, src, &mut files)?;
    let is_doc = |p: &Path| {
        let n = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        ZARR_DOCS.contains(&n.as_str())
            || Path::new(&n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
    };
    // An array directory holds `.zarray`, or a `zarr.json` with node_type array.
    let array_of = |rel: &Path| -> Option<PathBuf> {
        let mut d = rel.parent();
        while let Some(dir) = d {
            let full = src.join(dir);
            if full.join(".zarray").is_file() {
                return Some(dir.to_path_buf());
            }
            if let Ok(t) = std::fs::read_to_string(full.join("zarr.json"))
                && t.contains("\"array\"")
            {
                return Some(dir.to_path_buf());
            }
            d = dir.parent();
        }
        None
    };
    let mut docs = Vec::new();
    let mut by_array: std::collections::BTreeMap<PathBuf, (u64, Vec<PathBuf>)> =
        std::collections::BTreeMap::new();
    for f in files {
        if is_doc(&f) {
            docs.push(f);
            continue;
        }
        let arr = array_of(&f).unwrap_or_default();
        let size = std::fs::metadata(src.join(&f)).map_or(0, |m| m.len());
        let slot = by_array.entry(arr).or_default();
        slot.0 += size;
        slot.1.push(f);
    }
    // Metadata first (shallow documents before deep ones).
    docs.sort_by_key(|p| (p.components().count(), p.clone()));
    let mut steps: Vec<Step> = docs
        .into_iter()
        .map(|rel| Step {
            ops: vec![Op::File { rel }],
            kind: "metadata",
            unit: None,
        })
        .collect();
    // Level 0 (the largest array of each image) first, chunk by chunk; the rest after.
    let mut arrays: Vec<(PathBuf, (u64, Vec<PathBuf>))> = by_array.into_iter().collect();
    arrays.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let mut unit = 0u64;
    for (i, (_, (_, mut chunks))) in arrays.into_iter().enumerate() {
        chunks.sort_by_key(|p| chunk_key(p));
        for rel in chunks {
            steps.push(Step {
                ops: vec![Op::File { rel }],
                kind: if i == 0 { "chunk" } else { "metadata" },
                unit: (i == 0).then_some(unit),
            });
            if i == 0 {
                unit += 1;
            }
        }
    }
    Ok(Plan {
        src: src.to_path_buf(),
        pattern: Pattern::OmeZarr,
        steps,
        dir: true,
    })
}

/// Applies a [`Plan`] to a destination, one step at a time.
#[derive(Debug)]
pub struct Replayer {
    plan: Plan,
    dst: PathBuf,
    next: usize,
    src: Option<File>,
}

impl Replayer {
    /// Start a replay of `plan` into `dst`, which must not exist.
    pub fn new(plan: Plan, dst: &Path) -> Result<Replayer> {
        if dst.exists() {
            return Err(Error::Usage(format!(
                "{} exists; replay writes a new file",
                dst.display()
            )));
        }
        if plan.dir {
            std::fs::create_dir_all(dst).map_err(|e| Error::io(dst, e))?;
        }
        let src = if plan.dir {
            None
        } else {
            Some(File::open(&plan.src).map_err(|e| Error::io(&plan.src, e))?)
        };
        Ok(Replayer {
            plan,
            dst: dst.to_path_buf(),
            next: 0,
            src,
        })
    }

    /// The plan being replayed.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Steps applied so far.
    pub fn done(&self) -> usize {
        self.next
    }

    /// True when every step has been applied.
    pub fn finished(&self) -> bool {
        self.next >= self.plan.steps.len()
    }

    /// Apply the next step. Returns it, or `None` when the replay is finished.
    pub fn step(&mut self) -> Result<Option<&Step>> {
        let Some(step) = self.plan.steps.get(self.next).cloned() else {
            return Ok(None);
        };
        for op in &step.ops {
            self.apply(op)?;
        }
        self.next += 1;
        Ok(self.plan.steps.get(self.next - 1))
    }

    /// Apply steps up to and including index `last`.
    pub fn run_through(&mut self, last: usize) -> Result<()> {
        while self.next <= last && !self.finished() {
            self.step()?;
        }
        Ok(())
    }

    /// Apply every remaining step.
    pub fn finish(&mut self) -> Result<()> {
        while self.step()?.is_some() {}
        Ok(())
    }

    fn read_src(&mut self, off: u64, len: u64) -> Result<Vec<u8>> {
        let f = self
            .src
            .as_mut()
            .ok_or_else(|| Error::Other("no source file".into()))?;
        let n = usize::try_from(len).map_err(|_| corrupt("write too large"))?;
        let mut buf = vec![0u8; n];
        f.seek(SeekFrom::Start(off))
            .and_then(|_| f.read_exact(&mut buf))
            .map_err(|e| Error::io(&self.plan.src, e))?;
        Ok(buf)
    }

    fn apply(&mut self, op: &Op) -> Result<()> {
        match op {
            Op::At { rel, offset, data } => {
                let bytes = match data {
                    Data::Src(o, l) => self.read_src(*o, *l)?,
                    Data::Bytes(b) => b.clone(),
                    Data::Zeros(n) => vec![0u8; usize::try_from(*n).unwrap_or(0)],
                };
                let path = rel
                    .as_ref()
                    .map_or_else(|| self.dst.clone(), |r| self.dst.join(r));
                let mut f = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .open(&path)
                    .map_err(|e| Error::io(&path, e))?;
                f.seek(SeekFrom::Start(*offset))
                    .and_then(|_| f.write_all(&bytes))
                    .and_then(|()| f.flush())
                    .map_err(|e| Error::io(&path, e))?;
            }
            Op::File { rel } => {
                let from = self.plan.src.join(rel);
                let to = self.dst.join(rel);
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                let name = to
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let tmp = to.with_file_name(format!(".{name}.replay-partial"));
                std::fs::copy(&from, &tmp).map_err(|e| Error::io(&from, e))?;
                std::fs::rename(&tmp, &to).map_err(|e| Error::io(&to, e))?;
            }
        }
        Ok(())
    }

    /// Run the remaining steps, sleeping `interval` (divided by `speed`) before each unit
    /// step. `on_step` sees each applied step and the elapsed time.
    pub fn run_timed(
        &mut self,
        interval: Duration,
        speed: f64,
        on_step: &mut dyn FnMut(usize, &Step, Duration),
    ) -> Result<()> {
        let start = Instant::now();
        let pause = if speed > 0.0 && speed.is_finite() {
            interval.div_f64(speed)
        } else {
            interval
        };
        while !self.finished() {
            let is_unit = self.plan.steps[self.next].unit.is_some()
                || self.plan.steps[self.next].kind == "partial";
            if is_unit && self.next > 0 {
                std::thread::sleep(pause);
            }
            let i = self.next;
            let step = self.step()?.cloned();
            if let Some(s) = step {
                on_step(i, &s, start.elapsed());
            }
        }
        Ok(())
    }
}

/// xxh3-128 of a file, or of every file of a directory (path + contents, in path order):
/// replay tests compare the output with the source.
pub fn tree_hash(p: &Path) -> Result<String> {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    if p.is_dir() {
        let mut files = Vec::new();
        walk_files(p, p, &mut files)?;
        files.sort();
        for f in files {
            h.update(f.to_string_lossy().as_bytes());
            h.update(&read_all(&p.join(&f))?);
        }
    } else {
        h.update(&read_all(p)?);
    }
    Ok(format!("{:032x}", h.digest128()))
}
