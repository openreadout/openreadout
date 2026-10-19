//! Reference sequences for CRAM: a FASTA file found beside the CRAM file (or named by a local
//! `@SQ UR:` path), indexed by its `.fai` or by one pass over the file, and read one region at a
//! time. Bases are upper-cased, as the CRAM specification's MD5 rule does (§ 11).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use openreadout_core::source::Fs;

/// Where one sequence sits in the FASTA file.
#[derive(Debug, Clone)]
struct Entry {
    len: u64,
    offset: u64,
    line_bases: u64,
    line_bytes: u64,
}

/// An indexed FASTA file.
#[derive(Debug, Clone)]
pub struct Fasta {
    /// The FASTA file.
    pub path: PathBuf,
    fs: Fs,
    entries: HashMap<String, Entry>,
}

/// Parse a samtools `.fai` index: name, length, offset, bases per line, bytes per line.
fn parse_fai(text: &str) -> Option<HashMap<String, Entry>> {
    let mut m = HashMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 {
            return None;
        }
        let n = |i: usize| f[i].trim().parse::<u64>().ok();
        let e = Entry {
            len: n(1)?,
            offset: n(2)?,
            line_bases: n(3)?,
            line_bytes: n(4)?,
        };
        if e.line_bases == 0 || e.line_bytes < e.line_bases {
            return None;
        }
        m.insert(f[0].to_string(), e);
    }
    Some(m)
}

/// Index a FASTA file by reading it once. Every sequence must have lines of one length (the
/// last line may be shorter), as samtools faidx requires.
fn scan(fs: &Fs, path: &Path) -> Result<HashMap<String, Entry>, String> {
    let f = fs.open(path).map_err(|e| e.to_string())?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut m = HashMap::new();
    let mut at = 0u64;
    let mut line = Vec::new();
    let mut cur: Option<(String, Entry, bool)> = None;
    loop {
        line.clear();
        let n = (&mut r)
            .take(1 << 24)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let start = at;
        at += n as u64;
        if line.first() == Some(&b'>') {
            if let Some((name, e, _)) = cur.take() {
                m.insert(name, e);
            }
            let name = String::from_utf8_lossy(&line[1..])
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            cur = Some((
                name,
                Entry {
                    len: 0,
                    offset: at,
                    line_bases: 0,
                    line_bytes: 0,
                },
                false,
            ));
            continue;
        }
        let Some((_, e, short_seen)) = cur.as_mut() else {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Err("the reference FASTA has sequence before its first '>' line".into());
        };
        let bases = line
            .iter()
            .take_while(|b| **b != b'\n' && **b != b'\r')
            .count() as u64;
        if bases == 0 {
            *short_seen = true;
            continue;
        }
        if e.line_bases == 0 {
            e.line_bases = bases;
            e.line_bytes = n as u64;
            e.offset = start;
        } else if *short_seen
            || bases > e.line_bases
            || (bases < e.line_bases && n as u64 == e.line_bytes)
        {
            return Err(
                "the reference FASTA has lines of different lengths within one sequence; index it with `samtools faidx`"
                    .into(),
            );
        }
        if bases < e.line_bases {
            *short_seen = true;
        }
        e.len += bases;
    }
    if let Some((name, e, _)) = cur {
        m.insert(name, e);
    }
    Ok(m)
}

impl Fasta {
    /// Open and index `path` (its `.fai` when there is one).
    pub fn open(fs: &Fs, path: &Path) -> Result<Self, String> {
        let fai = PathBuf::from(format!("{}.fai", path.display()));
        let entries = match fs.read_to_string(&fai).ok().and_then(|t| parse_fai(&t)) {
            Some(e) => e,
            None => scan(fs, path)?,
        };
        Ok(Fasta {
            path: path.to_path_buf(),
            fs: fs.clone(),
            entries,
        })
    }

    /// The length of sequence `name`, when the file has it.
    pub fn len_of(&self, name: &str) -> Option<u64> {
        self.entries.get(name).map(|e| e.len)
    }

    /// Bases `start .. start + len` (0-based) of `name`, upper-cased; positions past the end of
    /// the sequence are left out.
    pub fn fetch(&self, name: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        let e = self
            .entries
            .get(name)
            .ok_or_else(|| format!("the reference FASTA has no sequence {name}"))?;
        let start = start.min(e.len);
        let end = start.saturating_add(len).min(e.len);
        if end <= start {
            return Ok(Vec::new());
        }
        let byte = |p: u64| e.offset + (p / e.line_bases) * e.line_bytes + p % e.line_bases;
        let (b0, b1) = (byte(start), byte(end - 1) + 1);
        let n = usize::try_from(b1 - b0).map_err(|_| "reference region too large".to_string())?;
        if n > 1 << 30 {
            return Err("reference region too large".into());
        }
        let f = self.fs.open(&self.path).map_err(|e| e.to_string())?;
        let mut raw = vec![0u8; n];
        f.read_exact_at(b0, &mut raw).map_err(|e| e.to_string())?;
        let mut out: Vec<u8> = raw
            .into_iter()
            .filter(|b| !b.is_ascii_whitespace())
            .map(|b| b.to_ascii_uppercase())
            .collect();
        out.truncate((end - start) as usize);
        Ok(out)
    }
}

/// MD5 of `bases` as 16 bytes.
pub(crate) fn md5(bases: &[u8]) -> [u8; 16] {
    use md5::Digest;
    let d = md5::Md5::digest(bases);
    let mut out = [0u8; 16];
    out.copy_from_slice(&d);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_fetch() {
        let fasta = b">a desc\nACGTA\ncgt\n>b\nTTTT\nTT\n".to_vec();
        let input = openreadout_core::source::Input::from_bytes("r.fa", fasta);
        let f = Fasta::open(input.fs(), input.path()).unwrap();
        assert_eq!(f.len_of("a"), Some(8));
        assert_eq!(f.fetch("a", 3, 4).unwrap(), b"TACG");
        assert_eq!(f.fetch("b", 4, 10).unwrap(), b"TT");
        assert!(f.fetch("c", 0, 1).is_err());
        assert_eq!(
            md5(b"ACGT"),
            [
                0xf1, 0xf8, 0xf4, 0xbf, 0x41, 0x3b, 0x16, 0xad, 0x13, 0x57, 0x22, 0xaa, 0x45, 0x91,
                0x04, 0x3e
            ]
        );
    }
}
