//! Personal-data detection in header metadata, and redaction with stable salted hashes.
//!
//! Precision over recall: a flag should nearly always be personal data, so every rule needs
//! either a field whose *name* says it holds a person (operator, owner, experimenter, ...), a
//! keyword next to the value (`MRN`, `DOB`, `patient`), or a pattern that is unambiguous on its
//! own (an e-mail address, an international phone number). The rules are listed in
//! `book/src/guides/lab-shares.md#personal-data`; each flag names the rule that fired.
//!
//! | kind | rule | fires on |
//! | --- | --- | --- |
//! | `person_name` | `operator_field` | a field named like a person (operator, owner, experimenter, investigator, user name, author, technician, analyst, scientist, acquired/created by) holding a name or account that is not a placeholder, a generic account (`admin`, `nmrsu`, `Administrator`, ...) or the instrument's own name |
//! | `person_name` | `operator_name_in_sample` | a sample field (sample id/name/well/barcode, image/table names, `*sample*`, `*specimen*`, `*subject*`, `*source*`, `*tube*` keys) containing an operator name flagged in the same file (whole words, 3+ letters) |
//! | `person_name` | `operator_name_in_field` | any other field containing every part (2+) of a flagged operator name (`Jane Roe 2013-02-28 Fortessa` for operator `JaneRoe`) |
//! | `person_name` | `patient_name` | `patient`/`subject name` followed by capitalised name words |
//! | `email` | `email_address` | `local@domain.tld` anywhere |
//! | `phone` | `phone_number` | `+` and 10–15 digits, or `(ddd) ddd-dddd` / `ddd-ddd-dddd` layouts; or 7–15 digits in a field named phone/tel/fax/mobile |
//! | `patient_id` | `mrn_keyword` | `MRN`, `patient id`, `medical record`, `hospital number`, `NHS number`, `PID` followed by an identifier with 3+ digits; or a field named like a patient/MRN id holding such an identifier |
//! | `date_of_birth` | `dob_keyword` | `DOB`, `date of birth`, `birth date`, `born`, `Geburtsdatum` followed by a date; or a field named like a birth date holding a date |
//! | `free_text` | `comment_field` | a comment/description/remark/annotation field with three or more words (may contain anything; flagged for review) |
//!
//! Values are never stored in the index: a flag is `{field, kind, rule}`. Redaction replaces a
//! flagged value with `redacted:` and the first 16 hex digits of HMAC-SHA256(salt, value), so
//! the same value gets the same replacement everywhere and across files (with the same salt),
//! and the salt comes from the user (never an argument, never logged).

use std::path::Path;

use openreadout_core::Error;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::record::PiiFlag;

/// A flag with the flagged value (kept in memory for redaction only; never written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Field path.
    pub field: String,
    /// Kind (`person_name`, `email`, ...).
    pub kind: &'static str,
    /// Rule id.
    pub rule: &'static str,
    /// The flagged value (the whole field).
    pub value: String,
}

impl Finding {
    /// The flag without its value.
    pub fn flag(&self) -> PiiFlag {
        PiiFlag {
            field: self.field.clone(),
            kind: self.kind.into(),
            rule: self.rule.into(),
        }
    }
}

/// Paths (prefixes) whose strings are ours, not the file's: never scanned.
const SKIP_PREFIXES: &[&str] = &[
    "path",
    "format",
    "notes",
    "experiment.provenance",
    "experiment.notes",
    "experiment.measurements",
    "experiment.method.technique",
    "experiment.method.assay",
    "experiment.instrument.kind",
    "experiment.sample.source_field",
];

/// Key names (lower case, spaces for separators) that hold a person.
const PERSON_KEYS: &[&str] = &[
    "operator",
    "owner",
    "experimenter",
    "investigator",
    "user name",
    "username",
    "user",
    "login",
    "author",
    "technician",
    "analyst",
    "scientist",
    "researcher",
    "acquired by",
    "created by",
    "submitter",
    "export user name",
    "principal investigator",
    "pi",
];

/// Values that are placeholders or shared accounts, not people.
const GENERIC: &[&str] = &[
    "",
    "-",
    "--",
    "n/a",
    "na",
    "none",
    "null",
    "nil",
    "unknown",
    "not set",
    "default",
    "user",
    "users",
    "<user>",
    "admin",
    "administrator",
    "administrators",
    "root",
    "guest",
    "operator",
    "system",
    "service",
    "lab",
    "labuser",
    "lab user",
    "public",
    "test",
    "tester",
    "demo",
    "nmrsu",
    "nmr",
    "topspin",
    "owner",
    "anonymous",
    "anon",
    "xxxx",
    "xxx",
    "default user",
    "default patient id",
    "sysadmin",
    "microscope",
    "instrument",
    "facility",
    "core",
    "customer",
    "student",
    "student1",
    "user1",
    "user01",
    "local",
    "localuser",
    "adm",
    "routine",
    "custom",
    "example",
    "public domain",
];

/// Words that make a value a role, shared account or organisation rather than a person.
const ROLE_WORDS: &[&str] = &[
    "user",
    "users",
    "group",
    "team",
    "lab",
    "laboratory",
    "facility",
    "core",
    "service",
    "admin",
    "administrator",
    "account",
    "routine",
    "custom",
    "default",
    "unknown",
    "example",
    "demo",
    "test",
    "guest",
    "public",
    "domain",
    "instrument",
    "system",
    "operator",
    "station",
    "university",
    "institute",
    "inc",
    "ltd",
    "gmbh",
    "corp",
    "company",
    "department",
    "dept",
    "copyright",
    "sample",
    "samples",
    "plate",
    "anonymous",
];

fn norm_key(k: &str) -> String {
    let mut s = String::with_capacity(k.len());
    for ch in k.chars() {
        if ch.is_alphanumeric() {
            s.extend(ch.to_lowercase());
        } else if !s.ends_with(' ') {
            s.push(' ');
        }
    }
    s.trim().to_string()
}

fn key_has(key: &str, words: &[&str]) -> bool {
    let k = format!(" {key} ");
    words.iter().any(|w| k.contains(&format!(" {w} ")))
}

fn is_generic(v: &str) -> bool {
    let l = v.trim().to_lowercase();
    GENERIC.contains(&l.as_str())
        || l.chars()
            .all(|c| c == 'x' || c == '*' || c == '?' || c == '-')
        || l.starts_with("default ")
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Does `v` read like a person's name or login (not software, not an instrument)?
fn plausible_person(v: &str, instrument_words: &[String]) -> bool {
    let v = v.trim();
    if v.len() < 2 || v.len() > 64 || is_generic(v) {
        return false;
    }
    if !v.chars().any(char::is_alphabetic) {
        return false;
    }
    let digits = v.chars().filter(char::is_ascii_digit).count();
    if digits * 3 > v.chars().count() {
        return false;
    }
    let lower = v.to_lowercase();
    for bad in [
        "software",
        "version",
        "windows",
        "microsoft",
        "instrument",
        "system",
        "server",
        "workstation",
        "controller",
        "acquisition",
        "default",
        "http",
        "www",
        "\\",
        "/",
    ] {
        if lower.contains(bad) {
            return false;
        }
    }
    let ws = words(v);
    if ws.is_empty() || ws.len() > 5 {
        return false;
    }
    // Two letters (`uk`, `SC`) are too short to tell a person from a code.
    if ws.len() == 1 && v.chars().count() < 3 {
        return false;
    }
    // Shared or role accounts and organisations: `ExactiveUser`, `TOF-User`, `Neumann_Group`,
    // `Unknown user`, `public domain`.
    if split_camel(v)
        .iter()
        .any(|w| ROLE_WORDS.contains(&w.to_lowercase().as_str()))
    {
        return false;
    }
    // Parameter and channel names (`V2-W`, `SSC-A`) and codes: a short token with digits or
    // a detector-style `XX-Y` pattern.
    if ws.len() <= 2 && v.len() <= 6 && (digits > 0 || v.contains('-')) {
        return false;
    }
    // The instrument's own name typed into the operator field ("LTQ OT Discovery").
    let shared = ws
        .iter()
        .filter(|w| instrument_words.iter().any(|i| i.eq_ignore_ascii_case(w)))
        .count();
    shared < 2 && !(ws.len() == 1 && shared == 1)
}

fn is_email_at(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let local = |c: u8| c.is_ascii_alphanumeric() || b"._%+-".contains(&c);
    let dom = |c: u8| c.is_ascii_alphanumeric() || c == b'.' || c == b'-';
    let mut s = at;
    while s > 0 && local(bytes[s - 1]) {
        s -= 1;
    }
    let mut e = at + 1;
    while e < bytes.len() && dom(bytes[e]) {
        e += 1;
    }
    if s == at || e == at + 1 {
        return None;
    }
    let domain = std::str::from_utf8(&bytes[at + 1..e])
        .ok()?
        .trim_end_matches('.');
    let (host, tld) = domain.rsplit_once('.')?;
    if host.is_empty() || tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((s, e))
}

/// Does the text contain an e-mail address?
pub fn has_email(text: &str) -> bool {
    let b = text.as_bytes();
    b.iter()
        .enumerate()
        .any(|(i, &c)| c == b'@' && is_email_at(b, i).is_some())
}

/// Does the text contain a phone number? `named_phone`: the field is named like one.
pub fn has_phone(text: &str, named_phone: bool) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !(c.is_ascii_digit() || c == '+' || c == '(') {
            i += 1;
            continue;
        }
        // Must not continue a word or number ("A12", "v1.2").
        if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '.') {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < chars.len()
            && (chars[j].is_ascii_digit() || " -.()+/".contains(chars[j]))
            && !(chars[j] == '+' && j > start)
        {
            j += 1;
        }
        let run: String = chars[start..j].iter().collect();
        let run = run.trim_end_matches([' ', '-', '.', '/', '(']);
        let digits = run.chars().filter(char::is_ascii_digit).count();
        let seps: Vec<char> = run
            .chars()
            .filter(|c| !c.is_ascii_digit() && *c != '+')
            .collect();
        let followed_by_word = chars.get(j).is_some_and(|c| c.is_alphabetic());
        i = j.max(i + 1);
        if followed_by_word || !(7..=15).contains(&digits) {
            continue;
        }
        if named_phone && digits >= 7 {
            return true;
        }
        if digits < 10 {
            continue;
        }
        // Dates, times, versions, IP addresses and decimals are not phone numbers.
        if seps.iter().all(|&s| s == '.' || s == ':') || run.contains(':') {
            continue;
        }
        let phone_seps = seps.iter().all(|&s| " -.()".contains(s));
        if run.starts_with('+') && phone_seps {
            return true;
        }
        if looks_like_date(run) || !phone_seps {
            continue;
        }
        let groups: Vec<usize> = run
            .split(|c: char| !c.is_ascii_digit())
            .filter(|g| !g.is_empty())
            .map(str::len)
            .collect();
        // (555) 019-9922, 555-019-9922, 555 019 9922
        if groups == [3, 3, 4]
            && (run.starts_with('(') || seps.iter().all(|&s| s == '-' || s == ' '))
        {
            return true;
        }
    }
    false
}

fn looks_like_date(s: &str) -> bool {
    let g: Vec<&str> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .collect();
    if g.len() < 3 {
        return false;
    }
    let n: Vec<usize> = g.iter().map(|x| x.len()).collect();
    matches!(n[..3], [4, 1 | 2, 1 | 2] | [1 | 2, 1 | 2, 4 | 2])
}

/// A date anywhere at the start of `s` (numeric with / - . separators, or `12 Mar 1980`).
fn starts_with_date(s: &str) -> bool {
    let s = s.trim_start();
    let head: String = s.chars().take(12).collect();
    if looks_like_date(&head) {
        return true;
    }
    let ws: Vec<&str> = s.split_whitespace().take(3).collect();
    if ws.len() == 3 {
        let month = ws[1].trim_end_matches(['.', ',']).to_lowercase();
        let months = [
            "jan",
            "feb",
            "mar",
            "apr",
            "may",
            "jun",
            "jul",
            "aug",
            "sep",
            "sept",
            "oct",
            "nov",
            "dec",
            "january",
            "february",
            "march",
            "april",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
        ];
        return ws[0]
            .trim_end_matches(['.', ','])
            .parse::<u32>()
            .is_ok_and(|d| (1..=31).contains(&d))
            && months.contains(&month.as_str())
            && ws[2].len() == 4
            && ws[2].parse::<u32>().is_ok();
    }
    false
}

/// Text after a keyword (case-insensitive, whole word), past `: # = -` and spaces.
fn after_keyword<'a>(text: &'a str, keywords: &[&str]) -> Vec<&'a str> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for k in keywords {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(k) {
            let at = from + pos;
            from = at + k.len();
            let before_ok = at == 0
                || !lower[..at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric);
            let after_ok = !lower[from..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
            if before_ok && after_ok && text.is_char_boundary(from) {
                out.push(text[from..].trim_start_matches([' ', ':', '#', '=', '-', '.', '\t']));
            }
        }
    }
    out
}

fn identifier_after(rest: &str) -> bool {
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '/')
        .collect();
    id.len() >= 4 && id.chars().filter(char::is_ascii_digit).count() >= 3
}

const MRN_KEYWORDS: &[&str] = &[
    "mrn",
    "patient id",
    "patient-id",
    "patient_id",
    "patientid",
    "patient no",
    "patient number",
    "medical record",
    "medical record number",
    "hospital number",
    "nhs number",
    "nhs no",
    "pid",
    "pat id",
    "pt id",
];

const DOB_KEYWORDS: &[&str] = &[
    "dob",
    "d.o.b",
    "date of birth",
    "birth date",
    "birthdate",
    "born",
    "born on",
    "geburtsdatum",
];

/// Does the text carry a patient identifier after an MRN-like keyword (or start with `MRN`
/// followed by digits)?
pub fn has_patient_id(text: &str) -> bool {
    if after_keyword(text, MRN_KEYWORDS)
        .iter()
        .any(|r| identifier_after(r))
    {
        return true;
    }
    // "MRN12345678"
    let lower = text.to_lowercase();
    lower.match_indices("mrn").any(|(i, _)| {
        let before_ok = i == 0
            || !lower[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_alphabetic);
        let rest = &lower[i + 3..];
        before_ok && rest.chars().take_while(char::is_ascii_digit).count() >= 5
    })
}

/// Does the text carry a date of birth after a DOB-like keyword?
pub fn has_date_of_birth(text: &str) -> bool {
    after_keyword(text, DOB_KEYWORDS)
        .iter()
        .any(|r| starts_with_date(r))
}

fn has_patient_name(text: &str) -> bool {
    after_keyword(
        text,
        &["patient", "patient name", "subject name", "pt name"],
    )
    .iter()
    .any(|rest| {
        let ws: Vec<&str> = rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty())
            .take(2)
            .collect();
        ws.len() == 2
            && ws.iter().all(|w| {
                let mut ch = w.chars();
                ch.next().is_some_and(char::is_uppercase)
                    && w.chars()
                        .skip(1)
                        .all(|c| c.is_lowercase() || c == '-' || c == '\'')
                    && w.chars().count() >= 2
            })
    })
}

/// `images[3].name` or `tables[0].name` (not `tables[0].columns[1].name`).
fn is_block_name(path: &str, block: &str) -> bool {
    path.strip_prefix(block)
        .and_then(|r| r.strip_prefix('['))
        .and_then(|r| r.strip_suffix("].name"))
        .is_some_and(|i| i.chars().all(|c| c.is_ascii_digit()))
}

fn is_sample_field(path: &str, key: &str) -> bool {
    path.starts_with("experiment.sample.")
        || is_block_name(path, "images")
        || is_block_name(path, "tables")
        || key_has(
            key,
            &[
                "sample", "specimen", "subject", "source", "src", "tube", "smno", "well",
            ],
        )
}

fn is_comment_field(path: &str, key: &str, raw_key: &str) -> bool {
    if path.starts_with("notes") {
        return false;
    }
    key_has(
        key,
        &[
            "comment",
            "comments",
            "description",
            "desc",
            "remark",
            "remarks",
            "annotation",
            "memo",
            "user comment",
        ],
    ) || matches!(
        raw_key
            .trim_start_matches('$')
            .to_ascii_uppercase()
            .as_str(),
        "COM" | "COMMENT"
    )
}

fn leaves(v: &Value, path: String, key: &str, out: &mut Vec<(String, String, String)>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                leaves(x, p, k, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                leaves(x, format!("{path}[{i}]"), key, out);
            }
        }
        Value::String(s) => out.push((path, key.to_string(), s.clone())),
        _ => {}
    }
}

fn skipped(path: &str) -> bool {
    SKIP_PREFIXES.iter().any(|p| {
        path == *p || path.starts_with(&format!("{p}.")) || path.starts_with(&format!("{p}["))
    })
}

fn instrument_words(doc: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |v: Option<&Value>| {
        if let Some(s) = v.and_then(Value::as_str) {
            out.extend(words(s).into_iter().map(str::to_string));
        }
    };
    let ins = doc.pointer("/experiment/instrument");
    push(ins.and_then(|i| i.get("model")));
    push(ins.and_then(|i| i.get("vendor")));
    push(ins.and_then(|i| i.get("software")));
    out
}

/// Scan an `info` JSON document (the file summary with its `experiment`), or any JSON tree
/// such as a vendor metadata tree, for personal data.
pub fn scan(doc: &Value) -> Vec<Finding> {
    let mut all = Vec::new();
    leaves(doc, String::new(), "", &mut all);
    let inst = instrument_words(doc);
    let mut out: Vec<Finding> = Vec::new();
    let push = |out: &mut Vec<Finding>,
                field: &str,
                kind: &'static str,
                rule: &'static str,
                value: &str| {
        if !out.iter().any(|f| f.field == field && f.kind == kind) {
            out.push(Finding {
                field: field.to_string(),
                kind,
                rule,
                value: value.to_string(),
            });
        }
    };
    let mut operators: Vec<String> = Vec::new();
    for (path, raw_key, value) in &all {
        if skipped(path) || value.trim().is_empty() {
            continue;
        }
        let key = norm_key(raw_key);
        let v = value.trim();
        if has_email(v) {
            push(&mut out, path, "email", "email_address", v);
        }
        if has_phone(
            v,
            key_has(
                &key,
                &["phone", "telephone", "tel", "fax", "mobile", "cell"],
            ),
        ) {
            push(&mut out, path, "phone", "phone_number", v);
        }
        let patient_key = key_has(
            &key,
            &["patient", "mrn", "patient id", "medical record", "nhs"],
        );
        if has_patient_id(v) || (patient_key && !is_generic(v) && identifier_after(v)) {
            push(&mut out, path, "patient_id", "mrn_keyword", v);
        }
        let birth_key = key_has(&key, &["dob", "birth", "birthdate", "birthday", "born"]);
        if has_date_of_birth(v) || (birth_key && starts_with_date(v)) {
            push(&mut out, path, "date_of_birth", "dob_keyword", v);
        }
        let person_key = path == "experiment.acquisition.operator"
            || (key_has(&key, PERSON_KEYS)
                && !key_has(
                    &key,
                    &[
                        "software",
                        "program",
                        "application",
                        "id",
                        "group",
                        "type",
                        "mode",
                        "count",
                        "number",
                        "level",
                        "defined",
                        "settings",
                        "name type",
                    ],
                )
                // Per-parameter keywords (`@MB_P10_USERNAME` is a user-defined channel name).
                && !key.split(' ').any(|w| {
                    w.len() > 1
                        && w.starts_with('p')
                        && w[1..].chars().all(|c| c.is_ascii_digit())
                }));
        if person_key && plausible_person(v, &inst) {
            push(&mut out, path, "person_name", "operator_field", v);
            if !operators.iter().any(|o| o.eq_ignore_ascii_case(v)) {
                operators.push(v.to_string());
            }
        }
        if (is_sample_field(path, &key) || patient_key) && has_patient_name(v) {
            push(&mut out, path, "person_name", "patient_name", v);
        }
        if is_comment_field(path, &key, raw_key) && words(v).len() >= 3 {
            push(&mut out, path, "free_text", "comment_field", v);
        }
    }
    // Operator names that reappear in sample fields.
    let name_words: Vec<String> = operators
        .iter()
        .flat_map(|o| split_camel(o))
        .filter(|w| {
            w.chars().count() >= 3
                && !is_generic(w)
                && !ROLE_WORDS.contains(&w.to_lowercase().as_str())
        })
        .map(|w| w.to_lowercase())
        .collect();
    // Full operator names (two or more name parts) anywhere else: an experiment named
    // "Jane Roe 2013-02-28 Fortessa".
    let full_names: Vec<Vec<String>> = operators
        .iter()
        .map(|o| name_parts(o))
        .filter(|p| p.len() >= 2)
        .collect();
    if !name_words.is_empty() {
        for (path, raw_key, value) in &all {
            if skipped(path)
                || out
                    .iter()
                    .any(|f| f.field == *path && f.kind == "person_name")
            {
                continue;
            }
            let value_words: Vec<String> = split_camel(value)
                .iter()
                .map(|w| w.to_lowercase())
                .collect();
            if full_names
                .iter()
                .any(|parts| parts.iter().all(|p| value_words.contains(p)))
            {
                push(
                    &mut out,
                    path,
                    "person_name",
                    "operator_name_in_field",
                    value.trim(),
                );
                continue;
            }
            if !is_sample_field(path, &norm_key(raw_key)) {
                continue;
            }
            let hit = value_words
                .iter()
                .any(|w| name_words.iter().any(|n| n.eq_ignore_ascii_case(w)));
            if hit {
                push(
                    &mut out,
                    path,
                    "person_name",
                    "operator_name_in_sample",
                    value.trim(),
                );
            }
        }
    }
    out.sort_by(|a, b| a.field.cmp(&b.field).then(a.kind.cmp(b.kind)));
    out
}

/// The parts of a name, lower case: words split at CamelCase humps (`JaneRoe` → `jane`,
/// `roe`), 3+ letters, not generic or role words.
fn name_parts(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in words(name) {
        for p in split_camel(w) {
            // split_camel also keeps the unsplit CamelCase word; only its parts count here.
            if p == w && w.chars().skip(1).any(char::is_uppercase) {
                continue;
            }
            let l = p.to_lowercase();
            if l.chars().count() >= 3
                && !is_generic(&l)
                && !ROLE_WORDS.contains(&l.as_str())
                && !out.contains(&l)
            {
                out.push(l);
            }
        }
    }
    out
}

/// Words of `s`, also splitting CamelCase (`JaneDoe` → `Jane`, `Doe`).
fn split_camel(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in words(s) {
        let mut cur = String::new();
        let mut prev_lower = false;
        for ch in w.chars() {
            if ch.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = ch.is_lowercase();
            cur.push(ch);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        if w.chars().any(char::is_lowercase) && w.chars().skip(1).any(char::is_uppercase) {
            out.push(w.to_string());
        }
    }
    out
}

/// Environment variable holding the redaction salt.
pub const SALT_ENV: &str = "OPENREADOUT_REDACT_SALT";

/// Shortest salt accepted, in bytes.
pub const MIN_SALT_LEN: usize = 8;

/// The redaction salt: the contents of `salt_file` (trailing newline removed), else the
/// [`SALT_ENV`] environment variable. The salt is never printed or written anywhere.
pub fn load_salt(salt_file: Option<&Path>) -> openreadout_core::Result<Vec<u8>> {
    let salt = if let Some(p) = salt_file {
        let mut b = std::fs::read(p).map_err(|e| Error::io(p, e))?;
        while b.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
            b.pop();
        }
        b
    } else if let Some(v) = std::env::var_os(SALT_ENV) {
        v.to_string_lossy().as_bytes().to_vec()
    } else {
        return Err(Error::Usage(format!(
            "--redact needs a salt: put a secret of at least {MIN_SALT_LEN} characters in a file and pass --salt-file FILE, or set {SALT_ENV}. The same salt gives the same replacements across runs; it is never printed or stored."
        )));
    };
    if salt.len() < MIN_SALT_LEN {
        return Err(Error::Usage(format!(
            "the redaction salt is shorter than {MIN_SALT_LEN} bytes; use a longer secret"
        )));
    }
    Ok(salt)
}

/// HMAC-SHA256 (RFC 2104).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Replaces flagged values with stable salted hashes.
pub struct Redactor {
    salt: Vec<u8>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Redactor { salt: <hidden> }")
    }
}

impl Redactor {
    /// A redactor keyed by `salt` (see [`load_salt`]).
    pub fn new(salt: Vec<u8>) -> Self {
        Redactor { salt }
    }

    /// The replacement for `value`: `redacted:` + 16 hex digits of HMAC-SHA256(salt, value).
    pub fn replacement(&self, value: &str) -> String {
        let mac = hmac_sha256(&self.salt, value.trim().as_bytes());
        let mut s = String::from("redacted:");
        for b in &mac[..8] {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Replace every flagged value in `doc`: the flagged fields themselves, and every other
    /// string that equals or contains a flagged value (4+ characters), so a name copied into
    /// another field or the vendor tree does not survive. Returns the number of strings changed.
    pub fn redact(&self, doc: &mut Value, findings: &[Finding]) -> usize {
        let mut values: Vec<&str> = findings
            .iter()
            .map(|f| f.value.trim())
            .filter(|v| v.chars().count() >= 4)
            .collect();
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        values.dedup();
        let fields: Vec<&str> = findings.iter().map(|f| f.field.as_str()).collect();
        let mut changed = 0;
        self.walk(doc, String::new(), &fields, &values, &mut changed);
        changed
    }

    fn walk(
        &self,
        v: &mut Value,
        path: String,
        fields: &[&str],
        values: &[&str],
        changed: &mut usize,
    ) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    self.walk(x, p, fields, values, changed);
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter_mut().enumerate() {
                    self.walk(x, format!("{path}[{i}]"), fields, values, changed);
                }
            }
            Value::String(s) => {
                if fields.contains(&path.as_str()) {
                    *s = self.replacement(s);
                    *changed += 1;
                    return;
                }
                let mut out = s.clone();
                for val in values {
                    if out.contains(val) {
                        out = out.replace(val, &self.replacement(val));
                    }
                }
                if out != *s {
                    *s = out;
                    *changed += 1;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(doc: &Value) -> Vec<(String, &'static str, &'static str)> {
        scan(doc)
            .into_iter()
            .map(|f| (f.field, f.kind, f.rule))
            .collect()
    }

    // Every name, number and address below is invented for the tests.
    #[test]
    fn operator_fields_are_flagged_unless_generic_or_the_instrument() {
        let doc = json!({
            "path": "/data/Jane_Roe/run1.raw",
            "tables": [{"extra": {"operator": "JaneRoe", "vendor_keywords": {"EXPORT": {"EXPORT USER NAME": "Administrator"}}}}],
            "experiment": {
                "instrument": {"model": "Orbitrap Exploris 480", "vendor": "Thermo"},
                "acquisition": {"operator": "Orbitrap Exploris"}
            }
        });
        let k = kinds(&doc);
        assert_eq!(
            k,
            vec![(
                "tables[0].extra.operator".into(),
                "person_name",
                "operator_field"
            )]
        );
        let doc = json!({"experiment": {"acquisition": {"operator": "nmrsu"}}});
        assert!(scan(&doc).is_empty());
        let doc = json!({"experiment": {"acquisition": {"operator": "j.roe"}}});
        assert_eq!(scan(&doc).len(), 1);
    }

    #[test]
    fn patterns_need_context() {
        let flagged = [
            (
                "comment",
                "contact jane.roe@example.org for access",
                "email",
            ),
            ("comment", "call +44 20 7946 0958", "phone"),
            ("sample", "MRN: 00123456", "patient_id"),
            ("sample", "MRN12345678", "patient_id"),
            ("title", "patient id 4455-12", "patient_id"),
            ("sample", "DOB 03/04/1971", "date_of_birth"),
            ("sample", "born 12 Mar 1980", "date_of_birth"),
            ("sample", "Patient Doe, John", "person_name"),
            ("phone", "555 0199 22", "phone"),
        ];
        for (key, v, kind) in flagged {
            let doc = json!({ "tables": [{"extra": { key: v }}] });
            let k: Vec<&str> = scan(&doc).iter().map(|f| f.kind).collect();
            assert!(k.contains(&kind), "{key}={v}: {k:?}");
        }
        let clean = [
            ("sample", "A12"),
            ("sample", "QC1_001"),
            ("sample", "PID controller gain 0.5"),
            ("acquired", "2020-05-03T10:11:12Z"),
            ("serial", "1234567890"),
            ("version", "1.2.3.4567.89"),
            ("comment", "ok"),
            ("sample", "Default Patient ID"),
            ("software", "user@localhost"),
            ("range", "(100-200) nm"),
            ("event", "Frame 1234567890123 at 12.5 s"),
            ("label", "Raw Data (480-14/520-30)"),
            ("operator", "ExactiveUser"),
            ("operator", "TOF-User"),
            ("operator", "Smith_Group"),
            ("operator", "Unknown user"),
            ("owner", "public domain"),
            ("owner", "uk"),
            ("@MB_P3_USERNAME", "V2-W"),
            ("GTI$USERNAME", "ADM"),
            ("supplemental note", "points at the primary TEXT segment"),
        ];
        for (key, v) in clean {
            let doc = json!({ "tables": [{"extra": { key: v }}] });
            assert!(scan(&doc).is_empty(), "{key}={v}: {:?}", scan(&doc));
        }
    }

    #[test]
    fn operator_names_in_sample_fields_and_free_text() {
        let doc = json!({
            "experiment": {
                "acquisition": {"operator": "Jane Roe"},
                "sample": {"id": "roe_mouse3_liver", "source_field": "tables[0].extra.specimen"}
            },
            "tables": [{"name": "plate 7", "extra": {"$COM": "second run after the clog, redo tomorrow"}}]
        });
        let k = kinds(&doc);
        assert!(k.contains(&(
            "experiment.sample.id".into(),
            "person_name",
            "operator_name_in_sample"
        )));
        assert!(k.contains(&("tables[0].extra.$COM".into(), "free_text", "comment_field")));
        assert!(!k.iter().any(|f| f.0 == "tables[0].name"));
        // The full name of a CamelCase operator, spelt with a space, elsewhere.
        let doc = json!({
            "tables": [{"extra": {
                "operator": "JaneRoe",
                "vendor_keywords": {"EXPERIMENT": {"EXPERIMENT NAME": "Jane Roe 2013-02-28 Fortessa"}},
                "cytometer": "Roe Fortessa"
            }}]
        });
        let k = kinds(&doc);
        assert!(k.contains(&(
            "tables[0].extra.vendor_keywords.EXPERIMENT.EXPERIMENT NAME".into(),
            "person_name",
            "operator_name_in_field"
        )));
        assert!(
            !k.iter().any(|f| f.0 == "tables[0].extra.cytometer"),
            "one name part outside a sample field is not enough"
        );
        let r = Redactor::new(b"a-test-salt".to_vec());
        let mut d = doc.clone();
        r.redact(&mut d, &scan(&doc));
        assert!(!d.to_string().contains("Jane Roe") && !d.to_string().contains("JaneRoe"));
        // Column names are not sample fields, even when they share a word with the operator.
        let doc = json!({
            "experiment": {"acquisition": {"operator": "Jane Roe"}},
            "tables": [{"name": "t", "columns": [{"name": "Roe-A"}]}]
        });
        assert_eq!(scan(&doc).len(), 1);
    }

    #[test]
    fn redaction_is_stable_and_salted() {
        let doc = json!({
            "experiment": {"acquisition": {"operator": "Jane Roe"}},
            "vendor": {"User": {"FullName": "Jane Roe (imaging core)", "Login": "jroe", "Home": "C:/Users/jroe/data"}}
        });
        let f = scan(&doc);
        let r = Redactor::new(b"a-test-salt".to_vec());
        let mut d1 = doc.clone();
        assert_eq!(r.redact(&mut d1, &f), 4);
        let rep = r.replacement("Jane Roe");
        assert!(rep.starts_with("redacted:") && rep.len() == 25);
        assert_eq!(d1["experiment"]["acquisition"]["operator"], json!(rep));
        // A field holding the full name is flagged and replaced as a whole.
        assert_eq!(
            d1["vendor"]["User"]["FullName"],
            json!(r.replacement("Jane Roe (imaging core)"))
        );
        // A flagged login inside an unflagged string is replaced in place.
        assert_eq!(
            d1["vendor"]["User"]["Home"],
            json!(format!("C:/Users/{}/data", r.replacement("jroe")))
        );
        assert!(!d1.to_string().contains("Jane Roe") && !d1.to_string().contains("jroe"));
        let other = Redactor::new(b"another-salt".to_vec());
        assert_ne!(other.replacement("Jane Roe"), rep);
        assert_eq!(
            Redactor::new(b"a-test-salt".to_vec()).replacement("Jane Roe"),
            rep
        );
        assert_eq!(format!("{r:?}"), "Redactor { salt: <hidden> }");
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex = mac.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        });
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
