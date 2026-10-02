//! Plane selection syntax shared by `export` and `check --planes`: `c=0`, `z=2-5`, `t=0,3,7`.

use crate::{Error, Result};

/// Which (c, z, t) planes to include. Empty axis list = all.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Channel indices; empty means every channel.
    pub c: Vec<u32>,
    /// Z indices; empty means every Z plane.
    pub z: Vec<u32>,
    /// Time indices; empty means every time point.
    pub t: Vec<u32>,
}

impl Selection {
    /// Parse repeatable `axis=spec` arguments.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut s = Selection::default();
        for a in args {
            let (axis, spec) = a.split_once('=').ok_or_else(|| {
                Error::Usage(format!(
                    "bad selection '{a}': expected axis=values, e.g. z=0-3"
                ))
            })?;
            let vals = parse_values(spec.trim())
                .map_err(|e| Error::Usage(format!("bad selection '{a}': {e}")))?;
            match axis.trim().to_ascii_lowercase().as_str() {
                "c" => s.c.extend(vals),
                "z" => s.z.extend(vals),
                "t" => s.t.extend(vals),
                other => {
                    return Err(Error::Usage(format!(
                        "unknown axis '{other}' in selection; use c, z or t"
                    )));
                }
            }
        }
        Ok(s)
    }

    /// True when plane (c, z, t) is selected.
    pub fn contains(&self, c: u32, z: u32, t: u32) -> bool {
        (self.c.is_empty() || self.c.contains(&c))
            && (self.z.is_empty() || self.z.contains(&z))
            && (self.t.is_empty() || self.t.contains(&t))
    }

    /// True when nothing is filtered out.
    pub fn is_all(&self) -> bool {
        self.c.is_empty() && self.z.is_empty() && self.t.is_empty()
    }
}

/// Most indices one axis selection may expand to (`z=0-4294967295` would otherwise
/// allocate 16 GiB). Far above any real plane count.
pub const MAX_SELECTED: usize = 1 << 20;

fn parse_values(spec: &str) -> std::result::Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            let a: u32 = a
                .trim()
                .parse()
                .map_err(|_| format!("'{a}' is not a number"))?;
            let b: u32 = b
                .trim()
                .parse()
                .map_err(|_| format!("'{b}' is not a number"))?;
            if b < a {
                return Err(format!("range {a}-{b} is reversed"));
            }
            if (b - a) as usize >= MAX_SELECTED - out.len().min(MAX_SELECTED) {
                return Err(format!(
                    "range {a}-{b} selects more than {MAX_SELECTED} indices"
                ));
            }
            out.extend(a..=b);
        } else {
            if out.len() >= MAX_SELECTED {
                return Err(format!("selection lists more than {MAX_SELECTED} indices"));
            }
            out.push(
                part.parse()
                    .map_err(|_| format!("'{part}' is not a number"))?,
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges_and_lists() {
        let s = Selection::parse(&["z=1-3".into(), "c=0,2".into()]).unwrap();
        assert_eq!(s.z, vec![1, 2, 3]);
        assert_eq!(s.c, vec![0, 2]);
        assert!(s.contains(0, 2, 9));
        assert!(!s.contains(1, 2, 0));
        assert!(Selection::parse(&["q=1".into()]).is_err());
    }

    #[test]
    fn huge_ranges_are_usage_errors_not_allocations() {
        let e = Selection::parse(&["z=0-4294967295".into()]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(Selection::parse(&["t=5-3".into()]).is_err());
        assert!(Selection::parse(&["c=".into()]).is_err());
        assert!(Selection::parse(&["c=-".into()]).is_err());
    }
}
