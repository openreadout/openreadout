//! Role names the layout could not place: the user's role map (`--role DMSO=negative`) and the
//! warnings about names that were read as samples or as controls of unknown sign
//! (book/src/guides/plate-analysis.md "Roles").

use std::collections::BTreeMap;

use openreadout_core::{Error, Result};

use crate::layout::{Layout, Role, WellInfo, norm, well_name};

/// A parsed role map: normalised name → role.
pub type RoleMap = Vec<(String, Role)>;

/// Parse `name → role` pairs (roles as [`Role::parse`] reads them).
pub fn parse_role_map(map: &BTreeMap<String, String>) -> Result<RoleMap> {
    let mut out = Vec::new();
    for (name, role) in map {
        let key = norm(name);
        if key.is_empty() {
            return Err(Error::Usage(format!(
                "role map: an empty name (`{name}={role}`)"
            )));
        }
        let r = Role::parse(role).ok_or_else(|| {
            Error::Usage(format!(
                "role map `{name}={role}`: `{role}` is not a role (use blank, standard, sample, positive, negative, control or empty)"
            ))
        })?;
        out.push((key, r));
    }
    Ok(out)
}

/// `ctl1` → `ctl`, `spl2:1` → `spl`: a name without its trailing replicate or level index.
fn stem(n: &str) -> &str {
    n.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':')
}

/// The role the map gives a well: its explicit role text, else its sample name, matched as
/// written or without a trailing index (`CTL` maps `CTL1`, `CTL2`).
fn mapped(info: &WellInfo, map: &RoleMap) -> Option<Role> {
    let names = [info.role_label.as_deref(), info.sample.as_deref()];
    for name in names.into_iter().flatten() {
        let n = norm(name);
        if let Some((_, r)) = map.iter().find(|(k, _)| *k == n) {
            return Some(*r);
        }
        let s = stem(&n);
        if !s.is_empty()
            && let Some((_, r)) = map.iter().find(|(k, _)| k == s)
        {
            return Some(*r);
        }
    }
    None
}

/// Apply the map to every well it names; returns a note per name used and a warning per map
/// entry that matched no well.
pub fn apply_role_map(lay: &mut Layout, map: &RoleMap) -> (Vec<String>, Vec<String>) {
    let mut used: BTreeMap<usize, usize> = BTreeMap::new();
    for info in lay.wells.values_mut() {
        if let Some(r) = mapped(info, map) {
            info.role = Some(r);
            info.role_mapped = true;
            let names = [info.role_label.as_deref(), info.sample.as_deref()];
            if let Some(k) = map.iter().position(|(k, _)| {
                names
                    .into_iter()
                    .flatten()
                    .any(|n| norm(n) == *k || stem(&norm(n)) == k)
            }) {
                *used.entry(k).or_default() += 1;
            }
        }
    }
    let notes = used
        .iter()
        .map(|(&k, n)| {
            format!(
                "role map: {n} well(s) named `{}` are {}",
                map[k].0,
                map[k].1.id()
            )
        })
        .collect();
    let unused = map
        .iter()
        .enumerate()
        .filter(|(k, _)| !used.contains_key(k))
        .map(|(_, (name, _))| format!("role map: no well of the layout is named `{name}`"))
        .collect();
    (notes, unused)
}

/// `NAME=ROLE` as a shell argument (quoted when the name has spaces).
fn arg(name: &str, role: &str) -> String {
    if name.contains(char::is_whitespace) {
        format!("'{name}={role}'")
    } else {
        format!("{name}={role}")
    }
}

fn well_list(wells: &[(u32, u32)]) -> String {
    let mut names: Vec<String> = wells
        .iter()
        .take(6)
        .map(|&(r, c)| well_name(r, c))
        .collect();
    if wells.len() > 6 {
        names.push("…".into());
    }
    names.join(", ")
}

/// Warnings about the role names of the wells measured (`(position, layout entry, role)`):
/// explicit role text no role matches (read as a sample), and controls of unknown sign.
pub fn warnings<'a>(
    wells: impl Iterator<Item = ((u32, u32), &'a WellInfo, Option<Role>)>,
) -> Vec<String> {
    let mut unknown: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut unsigned: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for (pos, info, role) in wells {
        if info.role_mapped {
            continue;
        }
        if let Some(label) = &info.role_label
            && Role::parse(label).is_none()
            && role == Some(Role::Sample)
        {
            unknown.entry(label.clone()).or_default().push(pos);
        }
        if role == Some(Role::Control) {
            let name = info
                .role_label
                .as_deref()
                .or(info.sample.as_deref())
                .unwrap_or("control");
            // `CTL1`, `CTL2` are one kind of control
            let s = stem(name.trim());
            let key = if s.is_empty() { name.trim() } else { s };
            unsigned.entry(key.to_string()).or_default().push(pos);
        }
    }
    let mut out = Vec::new();
    for (label, w) in &unknown {
        out.push(format!(
            "layout role `{label}` is not a role name ({} well(s): {}); read as sample. Name its role with --role {} (ROLE: blank, standard, sample, positive, negative, control, empty)",
            w.len(),
            well_list(w),
            arg(label, "ROLE")
        ));
    }
    for (name, w) in &unsigned {
        out.push(format!(
            "`{name}` wells ({}: {}) are controls of unspecified sign: left out of normalisation and Z′. If they are the no-effect (vehicle) or full-effect control, say so with --role {} or --role {}",
            w.len(),
            well_list(w),
            arg(name, "negative"),
            arg(name, "positive")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_and_warn() {
        let text = "Well,Type,Sample\nA1,DMSO,\nA2,DMSO,\nB1,inhibitor ctrl,\nB2,sample,X1\nC1,,CTL1\nC2,,CTL2\n";
        let mut lay = crate::layout::parse_layout(text, "t").unwrap();
        let wells = |lay: &Layout| -> Vec<((u32, u32), WellInfo, Option<Role>)> {
            lay.wells
                .iter()
                .map(|(p, i)| (*p, i.clone(), i.effective_role()))
                .collect()
        };
        let w = wells(&lay);
        let warn = warnings(w.iter().map(|(p, i, r)| (*p, i, *r)));
        assert_eq!(warn.len(), 3, "{warn:?}");
        assert!(
            warn.iter().any(|m| m.contains("`inhibitor ctrl`")),
            "{warn:?}"
        );
        assert!(warn.iter().any(|m| m.contains("`DMSO` wells")), "{warn:?}");
        assert!(
            warn.iter().any(|m| m.contains("`CTL` wells (2: C1, C2)")),
            "{warn:?}"
        );
        assert!(
            warn.iter().any(|m| m.contains("'inhibitor ctrl=ROLE'")),
            "{warn:?}"
        );
        let map = parse_role_map(&BTreeMap::from([
            ("dmso".to_string(), "negative".to_string()),
            ("CTL".to_string(), "pos".to_string()),
            ("nothing".to_string(), "blank".to_string()),
        ]))
        .unwrap();
        let (notes, unused) = apply_role_map(&mut lay, &map);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(unused.len(), 1);
        assert_eq!(lay.wells[&(0, 0)].effective_role(), Some(Role::Negative));
        assert_eq!(lay.wells[&(2, 1)].effective_role(), Some(Role::Positive));
        let w = wells(&lay);
        let warn = warnings(w.iter().map(|(p, i, r)| (*p, i, *r)));
        assert_eq!(warn.len(), 1, "{warn:?}");
        assert!(parse_role_map(&BTreeMap::from([("x".to_string(), "bogus".to_string())])).is_err());
    }
}
