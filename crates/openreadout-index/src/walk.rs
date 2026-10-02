//! A streaming, deterministic directory walk.
//!
//! Order: within a directory, its *items* (files, and directories a reader recognises as one
//! data set) come first, sorted by name, then its sub-directories, sorted by name, each walked
//! the same way. Every item has a [`Key`] (its path components, the last one marked as an
//! item); the walk yields items in increasing key order. The crawl relies on that order three
//! ways: the previous run's records are merged in by a single sequential pass (no lookup
//! table), a resumed run skips every item up to its last checkpoint without opening it (and
//! whole directories without listing them), and the tables come out sorted.
//!
//! Memory is one sorted listing per directory level being walked, not the size of the tree.
//! Hidden entries (`.name`) are skipped and counted; symbolic links are skipped and counted
//! unless followed (a directory reached twice through links is walked once).

use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use openreadout_core::Registry;
use serde::{Deserialize, Serialize};

use crate::record::ItemKind;

/// Is `dir` one data set (a walk item) rather than a directory to walk into? A directory a
/// reader recognises with less than definite confidence that holds recognised data-set
/// directories of its own (a Bruker project holding experiments `1/`, `2/`, …) is walked
/// into, so each experiment is its own data set. The same rule as recursive batch walks
/// ([`openreadout_core::batch::is_dataset_dir_in_walk`]).
pub fn is_dataset_dir(reg: &Registry, dir: &Path) -> bool {
    openreadout_core::batch::is_dataset_dir_in_walk(reg, dir)
}

/// A walk position: the path's components, each tagged `1` (a directory walked into) or `0`
/// (the item itself). Items of a directory sort before its sub-directories.
pub type Key = Vec<(u8, String)>;

fn components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
            Component::RootDir | Component::CurDir | Component::ParentDir => None,
        })
        .collect()
}

/// The key of an item (a file or a directory data set) at `path`.
pub fn item_key(path: &Path) -> Key {
    let mut c = components(path);
    let last = c.pop();
    let mut k: Key = c.into_iter().map(|s| (1, s)).collect();
    if let Some(l) = last {
        k.push((0, l));
    }
    k
}

/// The key prefix shared by everything under directory `path`.
pub fn dir_key(path: &Path) -> Key {
    components(path).into_iter().map(|s| (1, s)).collect()
}

/// Compare two item paths in walk order.
pub fn cmp_items(a: &Path, b: &Path) -> Ordering {
    item_key(a).cmp(&item_key(b))
}

/// Walk options.
#[derive(Debug, Clone, Default)]
pub struct WalkOptions {
    /// Follow symbolic links (files and directories).
    pub follow_symlinks: bool,
    /// Skip entries whose name, or whose path relative to their root, matches one of these.
    pub exclude: Vec<glob::Pattern>,
    /// Directories never walked into (the index directory itself).
    pub skip_dirs: Vec<PathBuf>,
}

/// Counters of what the walk skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WalkStats {
    /// Directories listed.
    pub directories: u64,
    /// Hidden entries (`.name`) skipped.
    pub hidden_skipped: u64,
    /// Symbolic links skipped (not followed).
    pub symlinks_skipped: u64,
    /// Entries skipped by `--exclude` (and the index directory).
    pub excluded: u64,
    /// Directories that could not be listed.
    pub unreadable_directories: u64,
}

/// One thing the walk found.
#[derive(Debug, Clone)]
pub enum WalkEvent {
    /// A file or directory data set to read.
    Item(WalkItem),
    /// A directory that could not be listed, or an entry that could not be examined.
    Error {
        /// The path.
        path: PathBuf,
        /// Why.
        message: String,
    },
}

/// A file or a directory data set.
#[derive(Debug, Clone)]
pub struct WalkItem {
    /// Absolute path.
    pub path: PathBuf,
    /// Index of the root it was found under.
    pub root: usize,
    /// File or directory data set.
    pub kind: ItemKind,
    /// File size (0 for directories: their members are summed when they are read).
    pub size: u64,
    /// Modification time, microseconds since the Unix epoch.
    pub mtime_us: Option<i64>,
}

/// Modification time of `meta` in microseconds since the Unix epoch.
pub fn mtime_us(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_micros()).ok(),
        Err(e) => i64::try_from(e.duration().as_micros()).ok().map(|v| -v),
    }
}

struct Frame {
    root: usize,
    items: VecDeque<(PathBuf, ItemKind, u64, Option<i64>)>,
    subdirs: VecDeque<PathBuf>,
}

/// The walk. An iterator of [`WalkEvent`]s in key order.
pub struct Walker<'a> {
    reg: &'a Registry,
    roots: Vec<PathBuf>,
    next_root: usize,
    stack: Vec<Frame>,
    pending: VecDeque<WalkEvent>,
    opts: WalkOptions,
    /// Skip items with a key up to and including this one (a resumed run).
    after: Option<Key>,
    visited: HashSet<PathBuf>,
    /// What was skipped.
    pub stats: WalkStats,
}

impl std::fmt::Debug for Walker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Walker")
            .field("roots", &self.roots)
            .field("depth", &self.stack.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Walker<'a> {
    /// Walk `roots` (absolute, sorted in key order, not nested in each other).
    pub fn new(reg: &'a Registry, roots: Vec<PathBuf>, opts: WalkOptions) -> Self {
        Walker {
            reg,
            roots,
            next_root: 0,
            stack: Vec::new(),
            pending: VecDeque::new(),
            opts,
            after: None,
            visited: HashSet::new(),
            stats: WalkStats::default(),
        }
    }

    /// Resume: yield only items after `key`.
    pub fn resume_after(&mut self, key: Key) {
        self.after = Some(key);
    }

    fn done_before(&self, key: &Key) -> bool {
        self.after.as_ref().is_some_and(|a| key <= a)
    }

    /// Is every item under directory `dir` at or before the resume point?
    fn subtree_done(&self, dir: &Path) -> bool {
        let Some(a) = &self.after else { return false };
        let prefix = dir_key(dir);
        prefix < *a && !a.starts_with(&prefix)
    }

    fn excluded(&self, root: usize, path: &Path, name: &str) -> bool {
        if self.opts.skip_dirs.iter().any(|d| d == path) {
            return true;
        }
        if self.opts.exclude.is_empty() {
            return false;
        }
        let rel = path
            .strip_prefix(&self.roots[root])
            .map_or_else(|_| path.to_string_lossy(), |r| r.to_string_lossy());
        self.opts
            .exclude
            .iter()
            .any(|p| p.matches(name) || p.matches(&rel))
    }

    /// List `dir` into a frame (items first, then sub-directories).
    fn enter(&mut self, root: usize, dir: &Path) -> Option<Frame> {
        if self.opts.follow_symlinks {
            let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            if !self.visited.insert(canon) {
                return None;
            }
        }
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                self.stats.unreadable_directories += 1;
                self.pending.push_back(WalkEvent::Error {
                    path: dir.to_path_buf(),
                    message: format!("cannot list directory: {e}"),
                });
                return None;
            }
        };
        self.stats.directories += 1;
        let mut items = Vec::new();
        let mut subdirs = Vec::new();
        for entry in rd {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    self.pending.push_back(WalkEvent::Error {
                        path: dir.to_path_buf(),
                        message: format!("cannot read a directory entry: {e}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                self.stats.hidden_skipped += 1;
                continue;
            }
            let path = entry.path();
            if self.excluded(root, &path, &name) {
                self.stats.excluded += 1;
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                self.pending.push_back(WalkEvent::Error {
                    path,
                    message: "cannot determine the entry type".into(),
                });
                continue;
            };
            let meta = if ft.is_symlink() {
                if !self.opts.follow_symlinks {
                    self.stats.symlinks_skipped += 1;
                    continue;
                }
                match std::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        self.pending.push_back(WalkEvent::Error {
                            path,
                            message: format!("broken symbolic link: {e}"),
                        });
                        continue;
                    }
                }
            } else if ft.is_dir() {
                // Directories need no metadata here; data-set directories are summed when read.
                subdirs.push(path);
                continue;
            } else {
                match entry.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        self.pending.push_back(WalkEvent::Error {
                            path,
                            message: format!("cannot stat: {e}"),
                        });
                        continue;
                    }
                }
            };
            if meta.is_dir() {
                subdirs.push(path);
            } else if meta.is_file() {
                items.push((path, ItemKind::File, meta.len(), mtime_us(&meta)));
            }
            // sockets, fifos, devices: not data
        }
        // Directory data sets are items of their parent.
        let mut plain = Vec::with_capacity(subdirs.len());
        for d in subdirs {
            if is_dataset_dir(self.reg, &d) {
                let m = std::fs::metadata(&d).ok();
                items.push((d, ItemKind::Directory, 0, m.as_ref().and_then(mtime_us)));
            } else {
                plain.push(d);
            }
        }
        let name_of = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        items.sort_by_key(|a| name_of(&a.0));
        plain.sort_by_key(|a| name_of(a));
        Some(Frame {
            root,
            items: items.into(),
            subdirs: plain.into(),
        })
    }

    fn start_root(&mut self, i: usize) {
        let root = self.roots[i].clone();
        let meta = match std::fs::metadata(&root) {
            Ok(m) => m,
            Err(e) => {
                self.pending.push_back(WalkEvent::Error {
                    path: root,
                    message: format!("cannot read the root: {e}"),
                });
                return;
            }
        };
        if meta.is_file() || is_dataset_dir(self.reg, &root) {
            let kind = if meta.is_file() {
                ItemKind::File
            } else {
                ItemKind::Directory
            };
            if !self.done_before(&item_key(&root)) {
                self.pending.push_back(WalkEvent::Item(WalkItem {
                    path: root,
                    root: i,
                    kind,
                    size: if meta.is_file() { meta.len() } else { 0 },
                    mtime_us: mtime_us(&meta),
                }));
            }
            return;
        }
        if self.subtree_done(&root) {
            return;
        }
        if let Some(f) = self.enter(i, &root) {
            self.stack.push(f);
        }
    }
}

impl Iterator for Walker<'_> {
    type Item = WalkEvent;

    fn next(&mut self) -> Option<WalkEvent> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                return Some(e);
            }
            let Some(top) = self.stack.last_mut() else {
                if self.next_root >= self.roots.len() {
                    return None;
                }
                let i = self.next_root;
                self.next_root += 1;
                self.start_root(i);
                continue;
            };
            if let Some((path, kind, size, mtime)) = top.items.pop_front() {
                let root = top.root;
                if self.done_before(&item_key(&path)) {
                    continue;
                }
                return Some(WalkEvent::Item(WalkItem {
                    path,
                    root,
                    kind,
                    size,
                    mtime_us: mtime,
                }));
            }
            if let Some(d) = top.subdirs.pop_front() {
                let root = top.root;
                if self.subtree_done(&d) {
                    continue;
                }
                if let Some(f) = self.enter(root, &d) {
                    self.stack.push(f);
                }
                continue;
            }
            self.stack.pop();
        }
    }
}

/// Every file under directory data set `dir` (hidden entries skipped, links not followed),
/// sorted by path: `(members, total bytes, newest mtime, total count)`. At most `cap`
/// members are listed; size, time and count cover all of them.
pub fn dir_members(dir: &Path, cap: usize) -> (Vec<crate::record::Member>, u64, Option<i64>, u64) {
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut newest: Option<i64> = None;
    let mut count = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(std::result::Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut subdirs = Vec::new();
        for e in entries {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                subdirs.push(e.path());
            } else if ft.is_file() {
                let Ok(m) = e.metadata() else { continue };
                let t = mtime_us(&m);
                total = total.saturating_add(m.len());
                count += 1;
                newest = match (newest, t) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                if out.len() < cap {
                    out.push(crate::record::Member {
                        path: e.path().to_string_lossy().into_owned(),
                        size: m.len(),
                        mtime_us: t,
                    });
                }
            }
        }
        // Depth-first in name order: push in reverse so the smallest name is walked first.
        for s in subdirs.into_iter().rev() {
            stack.push(s);
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, total, newest, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_put_items_before_subdirectories() {
        let a = item_key(Path::new("/r/zz.txt"));
        let b = item_key(Path::new("/r/aa/b.txt"));
        assert!(a < b, "items of /r come before /r/aa/…");
        assert!(item_key(Path::new("/r/a.txt")) < item_key(Path::new("/r/b.txt")));
        let d = dir_key(Path::new("/r/aa"));
        assert!(b.starts_with(&d));
    }

    #[test]
    fn walk_order_matches_key_order_and_resume_skips() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap();
        for f in [
            "z.txt",
            "a.txt",
            ".hidden",
            "sub/c.txt",
            "sub/deeper/d.txt",
            "sub/b.txt",
            "alpha/x.txt",
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let reg = Registry::new();
        let names = |w: Walker<'_>| -> Vec<String> {
            w.filter_map(|e| match e {
                WalkEvent::Item(i) => Some(
                    i.path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                ),
                WalkEvent::Error { .. } => None,
            })
            .collect()
        };
        let w = Walker::new(&reg, vec![root.clone()], WalkOptions::default());
        let all = names(w);
        assert_eq!(
            all,
            [
                "a.txt",
                "z.txt",
                "alpha/x.txt",
                "sub/b.txt",
                "sub/c.txt",
                "sub/deeper/d.txt"
            ]
        );
        let keys: Vec<Key> = all.iter().map(|n| item_key(&root.join(n))).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        let mut w = Walker::new(&reg, vec![root.clone()], WalkOptions::default());
        w.resume_after(item_key(&root.join("sub/b.txt")));
        assert_eq!(names(w), ["sub/c.txt", "sub/deeper/d.txt"]);
        let mut opts = WalkOptions::default();
        opts.exclude.push(glob::Pattern::new("sub").unwrap());
        let w = Walker::new(&reg, vec![root.clone()], opts);
        assert_eq!(names(w), ["a.txt", "z.txt", "alpha/x.txt"]);
    }
}
