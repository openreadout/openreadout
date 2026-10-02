//! Storage health report over an index: integrity, unreadable formats, exact duplicates,
//! near-duplicate exports, files at risk, totals and personal data.
//!
//! Everything comes from the index tables except the duplicate confirmation, which hashes the
//! full content (SHA-256) of the candidates only: data sets with the same size and content
//! fingerprint. Grouping works in partitions (by key hash) so memory stays bounded on very
//! large indexes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

use openreadout_core::Result;
use serde::Serialize;

use crate::fingerprint::{sha256_dir, sha256_file};
use crate::formats::{is_open_export_name, preservation, stem};
use crate::manifest::{EXPERIMENTS_FILE, FILES_FILE, PROBLEMS_FILE, Tally, read_manifest};
use crate::tables::{Cell, scan_table};

/// Options of `search --health`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HealthOptions {
    /// Confirm duplicate candidates by hashing their full content (reads those files).
    pub hash_duplicates: bool,
    /// Most entries listed per section (counts are always complete).
    pub max_list: usize,
    /// Rows per grouping partition (memory bound).
    pub partition_rows: u64,
}

impl Default for HealthOptions {
    fn default() -> Self {
        HealthOptions {
            hash_duplicates: true,
            max_list: 100,
            partition_rows: 2_000_000,
        }
    }
}

/// A count and bytes under a name.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct NamedTally {
    /// The format id, family, year or extension.
    pub name: String,
    /// How many.
    pub count: u64,
    /// Bytes.
    pub bytes: u64,
}

/// Totals.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct Totals {
    /// Data sets.
    pub datasets: u64,
    /// Bytes in data sets.
    pub dataset_bytes: u64,
    /// Files seen (all roles).
    pub files: u64,
    /// Files no reader recognises.
    pub unknown_files: u64,
    /// Their bytes.
    pub unknown_bytes: u64,
}

/// A data set with an integrity problem.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct IntegrityEntry {
    /// Path.
    pub path: String,
    /// Format id.
    pub format: Option<String>,
    /// `truncated`, `corrupt`, `unreadable` or `unsupported`.
    pub status: String,
    /// Finding or error codes.
    pub codes: Vec<String>,
    /// The error message, when it could not be opened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Integrity section.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct Integrity {
    /// Data sets per check status.
    pub status: BTreeMap<String, u64>,
    /// Data sets that are truncated, corrupt, unreadable or unsupported.
    pub problem_count: u64,
    /// The first of them.
    pub problems: Vec<IntegrityEntry>,
}

/// Formats recognised but not readable, per error.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct UnreadableFormat {
    /// Format id.
    pub format: String,
    /// Error code (`unsupported_feature`, `corrupt_file`, `io`).
    pub error_code: String,
    /// Data sets.
    pub count: u64,
}

/// Unreadable section.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct Unreadable {
    /// Recognised formats that could not be opened, per format and error.
    pub formats: Vec<UnreadableFormat>,
    /// Files no reader recognises, per extension (largest counts first).
    pub unknown_extensions: Vec<NamedTally>,
}

/// A group of identical data sets.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DuplicateGroup {
    /// Bytes of one copy.
    pub size_bytes: u64,
    /// SHA-256 of the content (null when not confirmed by hashing).
    pub sha256: Option<String>,
    /// The copies, in walk order.
    pub paths: Vec<String>,
}

/// Duplicates section.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct Duplicates {
    /// True when every group was confirmed by a full-content hash.
    pub confirmed: bool,
    /// Groups of identical data sets.
    pub group_count: u64,
    /// Bytes that extra copies take.
    pub redundant_bytes: u64,
    /// Candidate data sets hashed.
    pub candidates_hashed: u64,
    /// Bytes read to hash them.
    pub bytes_hashed: u64,
    /// The largest groups.
    pub groups: Vec<DuplicateGroup>,
}

/// The same experiment stored more than once (not byte-identical).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct NearDuplicateGroup {
    /// Why they were grouped: `same acquisition time and dimensions` or `same name and
    /// dimensions`.
    pub reason: String,
    /// The members, in walk order.
    pub paths: Vec<String>,
    /// Their formats, same order.
    pub formats: Vec<String>,
    /// True when two or more members are open exports (the experiment was exported twice).
    pub exported_twice: bool,
}

/// Near-duplicate section.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct NearDuplicates {
    /// Groups found.
    pub group_count: u64,
    /// Groups with two or more open exports.
    pub exported_twice: u64,
    /// The first groups.
    pub groups: Vec<NearDuplicateGroup>,
}

/// A data set at risk.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AtRiskEntry {
    /// Path.
    pub path: String,
    /// Format id.
    pub format: String,
    /// `vendor` or `legacy`.
    pub preservation: String,
    /// Bytes.
    pub size_bytes: u64,
}

/// At-risk section: vendor-only formats with no open export next to them.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct AtRisk {
    /// Data sets at risk.
    pub count: u64,
    /// Their bytes.
    pub bytes: u64,
    /// Of which in legacy formats (vendor software discontinued).
    pub legacy: u64,
    /// Per format.
    pub by_format: Vec<NamedTally>,
    /// The largest.
    pub datasets: Vec<AtRiskEntry>,
    /// Vendor/legacy data sets that do have an open export.
    pub with_open_export: u64,
}

/// Personal-data section (field paths and kinds only; never values).
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct PiiSection {
    /// Data sets with a flag.
    pub datasets_flagged: u64,
    /// Flags.
    pub flags: u64,
    /// Flags per kind.
    pub by_kind: BTreeMap<String, u64>,
    /// Flags per rule.
    pub by_rule: BTreeMap<String, u64>,
    /// Flags per field path (array indices removed), most frequent first.
    pub by_field: Vec<NamedTally>,
    /// The first flagged data sets with their kinds.
    pub datasets: Vec<PiiEntry>,
}

/// A flagged data set.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PiiEntry {
    /// Path.
    pub path: String,
    /// Kinds flagged.
    pub kinds: Vec<String>,
    /// Fields flagged.
    pub fields: Vec<String>,
}

/// `openreadout search INDEX --health`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HealthReport {
    /// The index directory.
    pub index_dir: String,
    /// Whether the index covers every root to the end.
    pub index_complete: bool,
    /// The roots of the index.
    pub roots: Vec<String>,
    /// When the index was last written.
    pub index_updated_at: String,
    /// When this report was made.
    pub generated_at: String,
    /// Totals.
    pub totals: Totals,
    /// Data sets and bytes per format.
    pub by_format: Vec<NamedTally>,
    /// Per family.
    pub by_family: Vec<NamedTally>,
    /// Per acquisition year.
    pub by_year: Vec<NamedTally>,
    /// Truncated and corrupt data sets.
    pub integrity: Integrity,
    /// Unreadable formats and unrecognised files.
    pub unreadable: Unreadable,
    /// Byte-identical copies.
    pub duplicates: Duplicates,
    /// The same experiment stored more than once.
    pub near_duplicates: NearDuplicates,
    /// Vendor-only formats with no open export.
    pub at_risk: AtRisk,
    /// Personal data.
    pub pii: PiiSection,
}

fn named(m: &BTreeMap<String, Tally>, by_count: bool) -> Vec<NamedTally> {
    let mut v: Vec<NamedTally> = m
        .iter()
        .map(|(k, t)| NamedTally {
            name: k.clone(),
            count: t.count,
            bytes: t.bytes,
        })
        .collect();
    if by_count {
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    }
    v
}

fn s(c: Cell) -> Option<String> {
    match c {
        Cell::Str(s) => Some(s),
        _ => None,
    }
}

fn u(c: &Cell) -> u64 {
    match c {
        Cell::U64(v) => *v,
        _ => 0,
    }
}

fn part_of(key: &impl Hash, parts: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish() % parts.max(1)
}

fn field_shape(f: &str) -> String {
    let mut out = String::with_capacity(f.len());
    let mut skip = false;
    for ch in f.chars() {
        match ch {
            '[' => {
                skip = true;
                out.push_str("[]");
            }
            ']' => skip = false,
            _ if skip => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Build the health report of the index in `index_dir`.
pub fn health(index_dir: &Path, opts: &HealthOptions) -> Result<HealthReport> {
    let m = read_manifest(index_dir)?;
    let exp = index_dir.join(EXPERIMENTS_FILE);
    let max = opts.max_list;
    let mut totals = Totals {
        datasets: m.datasets,
        files: m.files,
        ..Totals::default()
    };
    for t in m.unknown_extensions.values() {
        totals.unknown_files += t.count;
        totals.unknown_bytes += t.bytes;
    }
    // Pass A: integrity, unreadable formats.
    let mut integrity = Integrity::default();
    let mut unreadable_map: BTreeMap<(String, String), u64> = BTreeMap::new();
    scan_table(
        &exp,
        Some(&[
            "path",
            "format",
            "size_bytes",
            "check_status",
            "check_codes",
            "error_code",
            "error_message",
        ]),
        &mut |rows| {
            for r in 0..rows.len() {
                totals.dataset_bytes += u(&rows.get(r, "size_bytes"));
                let status = s(rows.get(r, "check_status")).unwrap_or_default();
                *integrity.status.entry(status.clone()).or_default() += 1;
                let err = s(rows.get(r, "error_code"));
                if let Some(e) = &err {
                    let f = s(rows.get(r, "format")).unwrap_or_default();
                    *unreadable_map.entry((f, e.clone())).or_default() += 1;
                }
                if matches!(
                    status.as_str(),
                    "truncated" | "corrupt" | "unreadable" | "unsupported"
                ) {
                    integrity.problem_count += 1;
                    if integrity.problems.len() < max {
                        let codes = match rows.get(r, "check_codes") {
                            Cell::List(l) => l,
                            _ => Vec::new(),
                        };
                        integrity.problems.push(IntegrityEntry {
                            path: s(rows.get(r, "path")).unwrap_or_default(),
                            format: s(rows.get(r, "format")),
                            status,
                            codes,
                            message: s(rows.get(r, "error_message")),
                        });
                    }
                }
            }
            Ok(())
        },
    )?;
    let mut unknown = named(&m.unknown_extensions, true);
    unknown.truncate(max.max(20));
    let unreadable = Unreadable {
        formats: unreadable_map
            .into_iter()
            .map(|((format, error_code), count)| UnreadableFormat {
                format,
                error_code,
                count,
            })
            .collect(),
        unknown_extensions: unknown,
    };
    // Open exports next to data sets: (dir, stem) of every open-format file.
    let mut open_stems: HashSet<(String, String)> = HashSet::new();
    let files = index_dir.join(FILES_FILE);
    scan_table(&files, Some(&["path", "role"]), &mut |rows| {
        for r in 0..rows.len() {
            if s(rows.get(r, "role")).as_deref() == Some("member") {
                continue;
            }
            let Some(p) = s(rows.get(r, "path")) else {
                continue;
            };
            let path = Path::new(&p);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if is_open_export_name(&name) {
                let dir = path
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                open_stems.insert((dir, stem(&name)));
            }
        }
        Ok(())
    })?;
    // Pass B (partitioned): exact duplicates and near duplicates.
    let parts = m.datasets.div_ceil(opts.partition_rows.max(1)).max(1);
    let mut dups = Duplicates {
        confirmed: opts.hash_duplicates,
        ..Duplicates::default()
    };
    let mut near = NearDuplicates::default();
    let mut protected: HashSet<String> = HashSet::new();
    let cols = [
        "path",
        "name",
        "dir",
        "kind",
        "format",
        "preservation",
        "size_bytes",
        "fingerprint",
        "started_at",
        "size_x",
        "size_y",
        "size_z",
        "size_c",
        "size_t",
        "image_count",
        "scan_count",
        "table_rows",
        "trace_samples",
    ];
    for part in 0..parts {
        let mut by_fp: HashMap<(u64, String), Vec<(String, bool)>> = HashMap::new();
        let mut by_exp: HashMap<String, Vec<(String, String, String, String)>> = HashMap::new();
        scan_table(&exp, Some(&cols), &mut |rows| {
            for r in 0..rows.len() {
                let path = s(rows.get(r, "path")).unwrap_or_default();
                let size = u(&rows.get(r, "size_bytes"));
                let format = s(rows.get(r, "format")).unwrap_or_default();
                let fp = s(rows.get(r, "fingerprint"));
                if let Some(fp) = &fp {
                    let key = (size, fp.clone());
                    if part_of(&key, parts) == part && size > 0 {
                        let is_dir = s(rows.get(r, "kind")).as_deref() == Some("directory");
                        by_fp.entry(key).or_default().push((path.clone(), is_dir));
                    }
                }
                let dims = format!(
                    "{}x{}x{}x{}x{}/{}i/{}s/{}r/{}t",
                    u(&rows.get(r, "size_x")),
                    u(&rows.get(r, "size_y")),
                    u(&rows.get(r, "size_z")),
                    u(&rows.get(r, "size_c")),
                    u(&rows.get(r, "size_t")),
                    u(&rows.get(r, "image_count")),
                    u(&rows.get(r, "scan_count")),
                    u(&rows.get(r, "table_rows")),
                    u(&rows.get(r, "trace_samples")),
                );
                if dims == "0x0x0x0x0/0i/0s/0r/0t" {
                    continue;
                }
                let fpv = fp.unwrap_or_default();
                if let Cell::Time(t) = rows.get(r, "started_at") {
                    let key = format!("time\u{1}{t}\u{1}{dims}");
                    if part_of(&key, parts) == part {
                        by_exp.entry(key).or_default().push((
                            path.clone(),
                            format.clone(),
                            fpv.clone(),
                            "same acquisition time and dimensions".into(),
                        ));
                    }
                }
                let dir = s(rows.get(r, "dir")).unwrap_or_default();
                let name = s(rows.get(r, "name")).unwrap_or_default();
                let key = format!("stem\u{1}{dir}\u{1}{}\u{1}{dims}", stem(&name));
                if part_of(&key, parts) == part {
                    by_exp.entry(key).or_default().push((
                        path,
                        format,
                        fpv,
                        "same name and dimensions".into(),
                    ));
                }
            }
            Ok(())
        })?;
        for ((size, _), paths) in by_fp {
            if paths.len() < 2 {
                continue;
            }
            // Confirm by full hash; split into groups of equal hashes.
            let mut groups: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
            for (p, is_dir) in paths {
                let h = if opts.hash_duplicates {
                    let res = if is_dir {
                        let (members, _, _, _) =
                            crate::walk::dir_members(Path::new(&p), usize::MAX);
                        sha256_dir(Path::new(&p), &members)
                    } else {
                        sha256_file(Path::new(&p))
                    };
                    match res {
                        Ok((h, n)) => {
                            dups.candidates_hashed += 1;
                            dups.bytes_hashed += n;
                            Some(h)
                        }
                        Err(_) => continue,
                    }
                } else {
                    None
                };
                groups.entry(h).or_default().push(p);
            }
            for (h, mut paths) in groups {
                if paths.len() < 2 {
                    continue;
                }
                paths.sort_by(|a, b| crate::walk::cmp_items(Path::new(a), Path::new(b)));
                dups.group_count += 1;
                dups.redundant_bytes += size * (paths.len() as u64 - 1);
                dups.groups.push(DuplicateGroup {
                    size_bytes: size,
                    sha256: h,
                    paths,
                });
            }
        }
        let mut seen_groups: HashSet<Vec<String>> = HashSet::new();
        for (_, mut members) in by_exp {
            if members.len() < 2 {
                continue;
            }
            let fps: HashSet<&String> = members.iter().map(|m| &m.2).collect();
            if fps.len() < 2 {
                continue; // byte-identical: reported as duplicates
            }
            members.sort_by(|a, b| crate::walk::cmp_items(Path::new(&a.0), Path::new(&b.0)));
            let paths: Vec<String> = members.iter().map(|m| m.0.clone()).collect();
            if !seen_groups.insert(paths.clone()) {
                continue;
            }
            let formats: Vec<String> = members.iter().map(|m| m.1.clone()).collect();
            let open = formats.iter().filter(|f| preservation(f) == "open").count();
            if open > 0 {
                protected.extend(paths.iter().cloned());
            }
            near.group_count += 1;
            if open >= 2 {
                near.exported_twice += 1;
            }
            near.groups.push(NearDuplicateGroup {
                reason: members[0].3.clone(),
                paths,
                formats,
                exported_twice: open >= 2,
            });
        }
    }
    dups.groups.sort_by(|a, b| {
        (b.size_bytes * b.paths.len() as u64)
            .cmp(&(a.size_bytes * a.paths.len() as u64))
            .then(a.paths.cmp(&b.paths))
    });
    dups.groups.truncate(max);
    near.groups.sort_by(|a, b| {
        b.exported_twice
            .cmp(&a.exported_twice)
            .then(a.paths.cmp(&b.paths))
    });
    near.groups.truncate(max);
    // Pass C: files at risk.
    let mut at_risk = AtRisk::default();
    let mut risk_formats: BTreeMap<String, Tally> = BTreeMap::new();
    scan_table(
        &exp,
        Some(&["path", "name", "dir", "format", "size_bytes"]),
        &mut |rows| {
            for r in 0..rows.len() {
                let format = s(rows.get(r, "format")).unwrap_or_default();
                let class = preservation(&format);
                if class == "open" {
                    continue;
                }
                let path = s(rows.get(r, "path")).unwrap_or_default();
                let dir = s(rows.get(r, "dir")).unwrap_or_default();
                let name = s(rows.get(r, "name")).unwrap_or_default();
                if protected.contains(&path) || open_stems.contains(&(dir, stem(&name))) {
                    at_risk.with_open_export += 1;
                    continue;
                }
                let size = u(&rows.get(r, "size_bytes"));
                at_risk.count += 1;
                at_risk.bytes += size;
                if class == "legacy" {
                    at_risk.legacy += 1;
                }
                risk_formats.entry(format.clone()).or_default().add(size);
                at_risk.datasets.push(AtRiskEntry {
                    path,
                    format,
                    preservation: class.into(),
                    size_bytes: size,
                });
                if at_risk.datasets.len() > max.saturating_mul(4).max(1000) {
                    at_risk
                        .datasets
                        .sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
                    at_risk.datasets.truncate(max);
                }
            }
            Ok(())
        },
    )?;
    at_risk
        .datasets
        .sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes).then(a.path.cmp(&b.path)));
    at_risk.datasets.truncate(max);
    let mut rf = named(&risk_formats, false);
    rf.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    at_risk.by_format = rf;
    // Pass D: personal data.
    let mut pii = PiiSection {
        datasets_flagged: m.pii.datasets_flagged,
        flags: m.pii.flags,
        by_kind: m.pii.by_kind.clone(),
        ..PiiSection::default()
    };
    let mut by_field: BTreeMap<String, u64> = BTreeMap::new();
    let mut current: Option<PiiEntry> = None;
    scan_table(
        &index_dir.join(PROBLEMS_FILE),
        Some(&["path", "category", "code", "field", "rule"]),
        &mut |rows| {
            for r in 0..rows.len() {
                if s(rows.get(r, "category")).as_deref() != Some("pii") {
                    continue;
                }
                let path = s(rows.get(r, "path")).unwrap_or_default();
                let kind = s(rows.get(r, "code")).unwrap_or_default();
                let field = s(rows.get(r, "field")).unwrap_or_default();
                *pii.by_rule
                    .entry(s(rows.get(r, "rule")).unwrap_or_default())
                    .or_default() += 1;
                *by_field.entry(field_shape(&field)).or_default() += 1;
                if current.as_ref().is_none_or(|c| c.path != path) {
                    if let Some(c) = current.take()
                        && pii.datasets.len() < max
                    {
                        pii.datasets.push(c);
                    }
                    current = Some(PiiEntry {
                        path,
                        kinds: Vec::new(),
                        fields: Vec::new(),
                    });
                }
                if let Some(c) = current.as_mut() {
                    if !c.kinds.contains(&kind) {
                        c.kinds.push(kind);
                    }
                    c.fields.push(field);
                }
            }
            Ok(())
        },
    )?;
    if let Some(c) = current.take()
        && pii.datasets.len() < max
    {
        pii.datasets.push(c);
    }
    let mut bf: Vec<NamedTally> = by_field
        .into_iter()
        .map(|(name, count)| NamedTally {
            name,
            count,
            bytes: 0,
        })
        .collect();
    bf.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    bf.truncate(max.max(20));
    pii.by_field = bf;
    let mut by_format = named(&m.formats, false);
    by_format.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    let mut by_family = named(&m.families, false);
    by_family.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    Ok(HealthReport {
        index_dir: index_dir.to_string_lossy().into_owned(),
        index_complete: m.complete,
        roots: m.roots.clone(),
        index_updated_at: m.updated_at.clone(),
        generated_at: crate::crawl::now_iso(),
        totals,
        by_format,
        by_family,
        by_year: named(&m.years, false),
        integrity,
        unreadable,
        duplicates: dups,
        near_duplicates: near,
        at_risk,
        pii,
    })
}

/// Bytes for people (`1.5 GB`).
pub fn human_bytes(b: u64) -> String {
    let units = ["B", "kB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", units[i])
    }
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

/// The report as Markdown.
pub fn render_markdown(r: &HealthReport) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    let _ = writeln!(o, "# Storage health report\n");
    let _ = writeln!(
        o,
        "Index `{}` of {} ({}; updated {}). Generated {}.\n",
        r.index_dir,
        r.roots
            .iter()
            .map(|x| format!("`{x}`"))
            .collect::<Vec<_>>()
            .join(", "),
        if r.index_complete {
            "complete"
        } else {
            "**incomplete crawl**"
        },
        r.index_updated_at,
        r.generated_at
    );
    let t = &r.totals;
    let _ = writeln!(o, "## Summary\n");
    let _ = writeln!(o, "| | |\n| --- | ---: |");
    let _ = writeln!(
        o,
        "| data sets | {} ({}) |",
        t.datasets,
        human_bytes(t.dataset_bytes)
    );
    let _ = writeln!(o, "| files seen | {} |", t.files);
    let _ = writeln!(
        o,
        "| unrecognised files | {} ({}) |",
        t.unknown_files,
        human_bytes(t.unknown_bytes)
    );
    let _ = writeln!(
        o,
        "| truncated, corrupt or unreadable | {} |",
        r.integrity.problem_count
    );
    let _ = writeln!(
        o,
        "| duplicate groups | {} ({} redundant{}) |",
        r.duplicates.group_count,
        human_bytes(r.duplicates.redundant_bytes),
        if r.duplicates.confirmed {
            ", confirmed by SHA-256"
        } else {
            ", not confirmed by hashing"
        }
    );
    let _ = writeln!(
        o,
        "| same experiment stored twice | {} ({} exported twice) |",
        r.near_duplicates.group_count, r.near_duplicates.exported_twice
    );
    let _ = writeln!(
        o,
        "| at risk (vendor-only, no open export) | {} ({}; {} legacy) |",
        r.at_risk.count,
        human_bytes(r.at_risk.bytes),
        r.at_risk.legacy
    );
    let _ = writeln!(
        o,
        "| data sets with possible personal data | {} ({} flags) |\n",
        r.pii.datasets_flagged, r.pii.flags
    );
    let table = |o: &mut String, title: &str, rows: &[NamedTally]| {
        let _ = writeln!(
            o,
            "## {title}\n\n| | data sets | bytes |\n| --- | ---: | ---: |"
        );
        for x in rows {
            let _ = writeln!(
                o,
                "| {} | {} | {} |",
                md_escape(&x.name),
                x.count,
                human_bytes(x.bytes)
            );
        }
        o.push('\n');
    };
    table(&mut o, "By format", &r.by_format);
    table(&mut o, "By family", &r.by_family);
    table(&mut o, "By acquisition year", &r.by_year);
    let _ = writeln!(o, "## Integrity\n");
    let _ = writeln!(
        o,
        "{}\n",
        r.integrity
            .status
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    if !r.integrity.problems.is_empty() {
        let _ = writeln!(
            o,
            "| path | format | status | codes |\n| --- | --- | --- | --- |"
        );
        for p in &r.integrity.problems {
            let _ = writeln!(
                o,
                "| `{}` | {} | {} | {} |",
                md_escape(&p.path),
                p.format.as_deref().unwrap_or("-"),
                p.status,
                md_escape(&p.codes.join(", "))
            );
        }
        o.push('\n');
    }
    let _ = writeln!(o, "## Unreadable\n");
    if !r.unreadable.formats.is_empty() {
        let _ = writeln!(o, "| format | error | data sets |\n| --- | --- | ---: |");
        for f in &r.unreadable.formats {
            let _ = writeln!(o, "| {} | {} | {} |", f.format, f.error_code, f.count);
        }
        o.push('\n');
    }
    if !r.unreadable.unknown_extensions.is_empty() {
        let _ = writeln!(
            o,
            "Unrecognised files by extension:\n\n| extension | files | bytes |\n| --- | ---: | ---: |"
        );
        for x in &r.unreadable.unknown_extensions {
            let _ = writeln!(
                o,
                "| {} | {} | {} |",
                md_escape(&x.name),
                x.count,
                human_bytes(x.bytes)
            );
        }
        o.push('\n');
    }
    let _ = writeln!(o, "## Duplicates\n");
    for g in &r.duplicates.groups {
        let _ = writeln!(
            o,
            "- {} × {}{}: {}",
            g.paths.len(),
            human_bytes(g.size_bytes),
            g.sha256
                .as_ref()
                .map(|h| format!(" (sha256 {})", &h[..16.min(h.len())]))
                .unwrap_or_default(),
            g.paths
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let _ = writeln!(o, "\n## Same experiment stored more than once\n");
    for g in &r.near_duplicates.groups {
        let _ = writeln!(
            o,
            "- {}{}: {}",
            g.reason,
            if g.exported_twice {
                ", **exported twice**"
            } else {
                ""
            },
            g.paths
                .iter()
                .zip(&g.formats)
                .map(|(p, f)| format!("`{p}` ({f})"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let _ = writeln!(o, "\n## Files at risk\n");
    let _ = writeln!(
        o,
        "Vendor-only formats with no open export (same name in the same directory, or the same experiment in an open format). {} data sets have one.\n",
        r.at_risk.with_open_export
    );
    if !r.at_risk.by_format.is_empty() {
        let _ = writeln!(
            o,
            "| format | class | data sets | bytes |\n| --- | --- | ---: | ---: |"
        );
        for x in &r.at_risk.by_format {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} |",
                x.name,
                preservation(&x.name),
                x.count,
                human_bytes(x.bytes)
            );
        }
        o.push('\n');
    }
    let _ = writeln!(o, "## Personal data\n");
    let _ = writeln!(
        o,
        "Flags name the field and the rule, never the value (https://openreadout.github.io/openreadout/guides/lab-shares.html#personal-data).\n"
    );
    if !r.pii.by_kind.is_empty() {
        let _ = writeln!(
            o,
            "{}\n",
            r.pii
                .by_kind
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(" · ")
        );
        let _ = writeln!(o, "| field | flags |\n| --- | ---: |");
        for f in &r.pii.by_field {
            let _ = writeln!(o, "| `{}` | {} |", md_escape(&f.name), f.count);
        }
    }
    o
}
