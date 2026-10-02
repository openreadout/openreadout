//! `Index.dat`: a five-character version string, the slide id, two little-endian `i32` pointers
//! (the hierarchical and non-hierarchical root tables), then pointer tables and linked lists of
//! data pages. Layout from OpenSlide's public format page and the corpus files
//! (`docs/formats/mirax.md` § Index file).

use std::collections::HashSet;

use openreadout_core::bytes::le_u32;

/// Largest number of items read from one record (a level of the largest corpus slide holds
/// about 90 000 images).
pub const MAX_RECORD_ITEMS: usize = 1 << 22;

/// One hierarchical data item: a pyramid image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HierItem {
    /// `y · IMAGENUMBER_X + x` of the image's first grid cell.
    pub image: u32,
    /// Byte offset in the data file.
    pub offset: u64,
    /// Byte length.
    pub length: u64,
    /// Data file number (`[DATAFILE] FILE_<n>`).
    pub file: u32,
}

/// One non-hierarchical data item (associated images, position tables, XML blocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonHierItem {
    /// The two leading words (usually 0).
    pub words: [i32; 2],
    /// Byte offset in the data file.
    pub offset: u64,
    /// Byte length.
    pub length: u64,
    /// Data file number.
    pub file: u32,
}

/// The parsed header of `Index.dat` and its raw bytes.
#[derive(Debug, Clone)]
pub struct IndexFile {
    /// The five-character version string (`01.02`).
    pub version: String,
    /// Offset of the hierarchical root table.
    pub hier_root: u32,
    /// Offset of the non-hierarchical root table.
    pub nonhier_root: u32,
    data: Vec<u8>,
}

/// A structural problem found in the index (reported by `check`, and as an error where it
/// stops a read).
#[derive(Debug, Clone, PartialEq)]
pub struct IndexProblem {
    /// Byte offset in `Index.dat`.
    pub at: u64,
    /// What is wrong.
    pub what: String,
}

impl IndexFile {
    /// Parse the header. `slide_id` is `[GENERAL] SLIDE_ID`; the root pointers follow it.
    pub fn parse(data: Vec<u8>, slide_id: &str) -> Result<IndexFile, IndexProblem> {
        let version = data
            .get(..5)
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .ok_or_else(|| IndexProblem {
                at: 0,
                what: "Index.dat is shorter than its 5-byte version string".into(),
            })?;
        let at = 5usize.saturating_add(slide_id.len());
        if data.get(5..at) != Some(slide_id.as_bytes()) {
            return Err(IndexProblem {
                at: 5,
                what: format!(
                    "Index.dat does not repeat the slide id {slide_id} of Slidedat.ini after its version"
                ),
            });
        }
        let (Some(hier_root), Some(nonhier_root)) = (le_u32(&data, at), le_u32(&data, at + 4))
        else {
            return Err(IndexProblem {
                at: at as u64,
                what: "Index.dat ends before its root pointers".into(),
            });
        };
        Ok(IndexFile {
            version,
            hier_root,
            nonhier_root,
            data,
        })
    }

    /// Size of the file in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the file is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn u32_at(&self, at: u64) -> Result<u32, IndexProblem> {
        usize::try_from(at)
            .ok()
            .and_then(|o| le_u32(&self.data, o))
            .ok_or_else(|| IndexProblem {
                at,
                what: format!(
                    "pointer to byte {at} lies outside Index.dat ({} bytes)",
                    self.data.len()
                ),
            })
    }

    /// The first page of record `k` of a root table (0: the record is empty).
    fn record_head(&self, root: u32, k: usize) -> Result<u32, IndexProblem> {
        let at = u64::from(root).saturating_add(4u64.saturating_mul(k as u64));
        self.u32_at(at)
    }

    /// Walk the linked list of record `k` under `root`, collecting items of `words` 4-byte
    /// words each.
    fn items(&self, root: u32, k: usize, words: usize) -> Result<Vec<Vec<u32>>, IndexProblem> {
        let mut page = self.record_head(root, k)?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        while page != 0 {
            if !seen.insert(page) {
                return Err(IndexProblem {
                    at: u64::from(page),
                    what: "the data-page list loops back on itself".into(),
                });
            }
            let count = self.u32_at(u64::from(page))?;
            let next = self.u32_at(u64::from(page) + 4)?;
            let count = count as usize;
            if out.len().saturating_add(count) > MAX_RECORD_ITEMS {
                return Err(IndexProblem {
                    at: u64::from(page),
                    what: format!("a record lists more than {MAX_RECORD_ITEMS} items"),
                });
            }
            let first = u64::from(page) + 8;
            let need = (count as u64).saturating_mul(4 * words as u64);
            if first.saturating_add(need) > self.data.len() as u64 {
                return Err(IndexProblem {
                    at: u64::from(page),
                    what: format!("a data page of {count} items runs past the end of Index.dat"),
                });
            }
            for i in 0..count as u64 {
                let base = first + i * 4 * words as u64;
                let mut item = Vec::with_capacity(words);
                for w in 0..words as u64 {
                    item.push(self.u32_at(base + 4 * w)?);
                }
                out.push(item);
            }
            page = next;
        }
        Ok(out)
    }

    /// Items of hierarchical record `k` (records of all hierarchies, in `HIER_0`, `HIER_1`, …
    /// value order).
    pub fn hier_items(&self, k: usize) -> Result<Vec<HierItem>, IndexProblem> {
        Ok(self
            .items(self.hier_root, k, 4)?
            .into_iter()
            .map(|w| HierItem {
                image: w[0],
                offset: u64::from(w[1]),
                length: u64::from(w[2]),
                file: w[3],
            })
            .collect())
    }

    /// Items of non-hierarchical record `k`.
    pub fn nonhier_items(&self, k: usize) -> Result<Vec<NonHierItem>, IndexProblem> {
        Ok(self
            .items(self.nonhier_root, k, 5)?
            .into_iter()
            .map(|w| NonHierItem {
                words: [w[0] as i32, w[1] as i32],
                offset: u64::from(w[2]),
                length: u64::from(w[3]),
                file: w[4],
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An index with one hierarchical record (two pages: the empty head, then 2 items) and one
    /// non-hierarchical record (1 item), laid out as in the corpus files.
    pub(crate) fn sample(slide_id: &str) -> Vec<u8> {
        let mut d = b"01.02".to_vec();
        d.extend_from_slice(slide_id.as_bytes());
        let roots_at = d.len();
        d.extend_from_slice(&[0; 8]);
        let w = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        // hierarchical root table: one record
        let hroot = d.len() as u32;
        w(&mut d, 0);
        // non-hierarchical root table: one record
        let nroot = d.len() as u32;
        w(&mut d, 0);
        // hier list: empty head page -> page with 2 items
        let head = d.len() as u32;
        w(&mut d, 0);
        let next_at = d.len();
        w(&mut d, 0);
        let page = d.len() as u32;
        w(&mut d, 2);
        w(&mut d, 0);
        for (img, off, len, file) in [(0u32, 296u32, 100u32, 0u32), (5, 396, 50, 1)] {
            w(&mut d, img);
            w(&mut d, off);
            w(&mut d, len);
            w(&mut d, file);
        }
        d[next_at..next_at + 4].copy_from_slice(&page.to_le_bytes());
        d[hroot as usize..hroot as usize + 4].copy_from_slice(&head.to_le_bytes());
        // nonhier list: head -> 1 item
        let nhead = d.len() as u32;
        w(&mut d, 0);
        let nnext_at = d.len();
        w(&mut d, 0);
        let npage = d.len() as u32;
        w(&mut d, 1);
        w(&mut d, 0);
        for v in [0u32, 0, 296, 9, 2] {
            w(&mut d, v);
        }
        d[nnext_at..nnext_at + 4].copy_from_slice(&npage.to_le_bytes());
        d[nroot as usize..nroot as usize + 4].copy_from_slice(&nhead.to_le_bytes());
        d[roots_at..roots_at + 4].copy_from_slice(&hroot.to_le_bytes());
        d[roots_at + 4..roots_at + 8].copy_from_slice(&nroot.to_le_bytes());
        d
    }

    #[test]
    fn walks_records() {
        let idx = IndexFile::parse(sample("SLIDE1"), "SLIDE1").unwrap();
        assert_eq!(idx.version, "01.02");
        let h = idx.hier_items(0).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(
            h[1],
            HierItem {
                image: 5,
                offset: 396,
                length: 50,
                file: 1
            }
        );
        let n = idx.nonhier_items(0).unwrap();
        assert_eq!((n[0].offset, n[0].length, n[0].file), (296, 9, 2));
        // a record beyond the table reads whatever follows; out of the file is an error
        assert!(idx.hier_items(1_000_000).is_err());
        assert!(IndexFile::parse(sample("SLIDE1"), "OTHER").is_err());
    }

    #[test]
    fn loops_and_truncation_are_errors() {
        let mut d = sample("S");
        let idx = IndexFile::parse(d.clone(), "S").unwrap();
        let head = idx.record_head(idx.hier_root, 0).unwrap() as usize;
        // point the head page's "next" at itself
        d[head + 4..head + 8].copy_from_slice(&(head as u32).to_le_bytes());
        let idx = IndexFile::parse(d, "S").unwrap();
        assert!(idx.hier_items(0).unwrap_err().what.contains("loops"));
        let short = sample("S")[..20].to_vec();
        let idx = IndexFile::parse(short, "S").unwrap();
        assert!(idx.hier_items(0).is_err());
    }
}
