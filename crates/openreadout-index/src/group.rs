//! Multi-file data-set grouping, shared by the index and batch tables.
//!
//! A data set that names other files ([`openreadout_core::Dataset::member_files`]: multi-file
//! OME-TIFF, CZI parts, OIR continuations, SpikeGLX `.meta`/`.bin`, imzML `.ibd`, VSI `.ets`,
//! LIF sidecars) *claims* them. Files linked by claims form one component; its data set is the
//! claimer that opened cleanly and names the most files, the first in walk order on a tie. Every
//! other file of the component is a member of that data set.

use std::collections::HashMap;

/// One data set that names other files.
#[derive(Debug, Clone, Default)]
pub struct Claim {
    /// The claiming file.
    pub path: String,
    /// Whether it opened cleanly.
    pub ok: bool,
    /// The files it names (other than itself).
    pub members: Vec<String>,
    /// Position in walk order (smaller first).
    pub seq: u64,
}

struct Uf {
    parent: Vec<usize>,
}

impl Uf {
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b] = a;
        }
    }
}

/// Map every path of a claimed component that is not its data set to that data set's path.
/// Paths are compared as given (callers pass absolute or canonical paths consistently).
pub fn primaries(claims: &[Claim]) -> HashMap<String, String> {
    let node = |p: &str, ids: &mut HashMap<String, usize>, names: &mut Vec<String>| -> usize {
        if let Some(&i) = ids.get(p) {
            return i;
        }
        let i = names.len();
        names.push(p.to_string());
        ids.insert(p.to_string(), i);
        i
    };
    let mut owned_ids: HashMap<String, usize> = HashMap::new();
    let mut owned_names: Vec<String> = Vec::new();
    let mut edges = Vec::new();
    let mut claimer_of: Vec<(usize, &Claim)> = Vec::new();
    for c in claims {
        let a = node(&c.path, &mut owned_ids, &mut owned_names);
        claimer_of.push((a, c));
        for m in &c.members {
            let b = node(m, &mut owned_ids, &mut owned_names);
            edges.push((a, b));
        }
    }
    let mut uf = Uf {
        parent: (0..owned_names.len()).collect(),
    };
    for (a, b) in edges {
        uf.union(a, b);
    }
    let mut best: HashMap<usize, (usize, &Claim)> = HashMap::new();
    for &(n, c) in &claimer_of {
        let root = uf.find(n);
        let better = match best.get(&root) {
            None => true,
            Some((_, b)) => {
                (c.ok, c.members.len(), std::cmp::Reverse(c.seq))
                    > (b.ok, b.members.len(), std::cmp::Reverse(b.seq))
            }
        };
        if better {
            best.insert(root, (n, c));
        }
    }
    let mut out = HashMap::new();
    for n in 0..owned_names.len() {
        let root = uf.find(n);
        if let Some((primary, _)) = best.get(&root)
            && *primary != n
        {
            out.insert(owned_names[n].clone(), owned_names[*primary].clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(path: &str, members: &[&str], seq: u64) -> Claim {
        Claim {
            path: path.into(),
            ok: true,
            members: members.iter().map(|s| (*s).to_string()).collect(),
            seq,
        }
    }

    #[test]
    fn the_claimer_naming_most_files_wins_then_walk_order() {
        let m = primaries(&[
            claim("b.ome.tif", &["a.ome.tif"], 1),
            claim("a.ome.tif", &["b.ome.tif", "c.ome.tif"], 2),
            claim("x.czi", &["x(1).czi"], 3),
            claim("x(1).czi", &["x.czi"], 4),
        ]);
        assert_eq!(m["b.ome.tif"], "a.ome.tif");
        assert_eq!(m["c.ome.tif"], "a.ome.tif");
        assert!(!m.contains_key("a.ome.tif"));
        assert_eq!(m["x(1).czi"], "x.czi");
        assert!(!m.contains_key("x.czi"));
    }

    #[test]
    fn a_claimer_that_failed_loses_to_one_that_opened() {
        let mut bad = claim("a", &["b", "c"], 1);
        bad.ok = false;
        let m = primaries(&[bad, claim("b", &["a"], 2)]);
        assert_eq!(m["a"], "b");
        assert_eq!(m["c"], "b");
    }
}
