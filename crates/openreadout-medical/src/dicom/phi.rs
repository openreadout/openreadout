//! Patient-identifying elements (PS3.15 Annex E, Basic Application Level Confidentiality
//! Profile). Their values are never printed: `info` lists which ones a file holds under
//! `extra.personal_data` (field and kind, no value) and the vendor tree shows `withheld`.

use serde_json::{Value, json};

use super::dictionary::{lookup, tag_text};
use super::element::Lookup;
use super::parse::{Element, Item, Value as V};
use super::tags as t;

/// What the vendor tree shows instead of a withheld value.
pub(crate) const WITHHELD: &str = "withheld";

/// Elements that identify the patient (or another person) by value, with the kind used by
/// the personal-data flags of `openreadout index` (`person_name`, `patient_id`,
/// `date_of_birth`, `address`, `phone`, `free_text`). Every PN (person name) element is
/// withheld as well.
static IDENTIFYING: &[(u32, &str)] = &[
    (0x0008_0050, "patient_id"),    // AccessionNumber
    (0x0008_0081, "address"),       // InstitutionAddress
    (0x0008_0092, "address"),       // ReferringPhysicianAddress
    (0x0008_0094, "phone"),         // ReferringPhysicianTelephoneNumbers
    (0x0010_0020, "patient_id"),    // PatientID
    (0x0010_0021, "patient_id"),    // IssuerOfPatientID
    (0x0010_0030, "date_of_birth"), // PatientBirthDate
    (0x0010_0032, "date_of_birth"), // PatientBirthTime
    (0x0010_0050, "patient_id"),    // PatientInsurancePlanCodeSequence
    (0x0010_1000, "patient_id"),    // OtherPatientIDs
    (0x0010_1002, "patient_id"),    // OtherPatientIDsSequence
    (0x0010_1040, "address"),       // PatientAddress
    (0x0010_1080, "patient_id"),    // MilitaryRank
    (0x0010_1081, "patient_id"),    // BranchOfService
    (0x0010_1090, "patient_id"),    // MedicalRecordLocator
    (0x0010_2150, "address"),       // CountryOfResidence
    (0x0010_2152, "address"),       // RegionOfResidence
    (0x0010_2154, "phone"),         // PatientTelephoneNumbers
    (0x0010_2155, "phone"),         // PatientTelecomInformation
    (0x0010_21B0, "free_text"),     // AdditionalPatientHistory
    (0x0010_4000, "free_text"),     // PatientComments
    (0x0038_0010, "patient_id"),    // AdmissionID
    (0x0038_0400, "address"),       // PatientInstitutionResidence
    (0x0040_1103, "phone"),         // PersonTelephoneNumbers
    (0x0040_1102, "address"),       // PersonAddress
    (0x0040_A123, "person_name"),   // PersonName
];

/// The kind of personal data an element holds, when it is one to withhold.
pub(crate) fn kind_of(e: &Element) -> Option<&'static str> {
    if &e.vr == b"PN" {
        return Some("person_name");
    }
    IDENTIFYING
        .iter()
        .find(|(tag, _)| *tag == e.tag)
        .map(|(_, k)| *k)
}

fn has_value(e: &Element) -> bool {
    match &e.value {
        V::Bytes(b) => b.iter().any(|c| !matches!(c, b' ' | 0)),
        V::Seq(items) => !items.is_empty(),
        V::Deferred { len, .. } => *len > 0,
        V::Encapsulated { .. } => false,
    }
}

/// Every identifying element in `ds` (sequences included) that holds a value:
/// `{field, kind}` with the field as `Keyword (gggg,eeee)`, each field listed once.
pub(crate) fn flags(ds: &Item) -> Vec<Value> {
    let mut seen: Vec<(u32, &'static str)> = Vec::new();
    walk(ds, &mut seen, 0);
    let mut out: Vec<Value> = seen
        .into_iter()
        .map(|(tag, kind)| {
            let name = lookup(tag)
                .map_or_else(|| tag_text(tag), |(k, _)| format!("{k} {}", tag_text(tag)));
            json!({"field": name, "kind": kind})
        })
        .collect();
    if ds
        .text(t::BURNED_IN_ANNOTATION)
        .is_some_and(|v| v.eq_ignore_ascii_case("YES"))
    {
        out.push(json!({
            "field": format!("BurnedInAnnotation {}", tag_text(t::BURNED_IN_ANNOTATION)),
            "kind": "burned_in_text",
        }));
    }
    out
}

fn walk(items: &Item, seen: &mut Vec<(u32, &'static str)>, depth: u32) {
    if depth > 16 {
        return;
    }
    for e in items {
        if let Some(k) = kind_of(e)
            && has_value(e)
            && !seen.iter().any(|(t, _)| *t == e.tag)
        {
            seen.push((e.tag, k));
        }
        if let V::Seq(v) = &e.value {
            for it in v {
                walk(it, seen, depth + 1);
            }
        }
    }
}

/// True when the file says identifying information was removed (Patient Identity Removed,
/// (0012,0062) = YES).
pub(crate) fn deidentified(ds: &Item) -> bool {
    ds.text(0x0012_0062)
        .is_some_and(|v| v.eq_ignore_ascii_case("YES"))
}
