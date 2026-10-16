//! `RunInfo.xml` (run id, flow cell, instrument, reads and their cycles, flow-cell layout) and
//! `RunParameters.xml` (control software, RTA version, experiment name, reagent kits): the two
//! XML files an Illumina instrument writes at the top of every run folder.

use roxmltree::{Document, Node};

use super::FORMAT_ID;
use openreadout_core::{Error, Result};

/// One read of the run (a sequencing read or an index read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadInfo {
    /// Read number as `RunInfo.xml` numbers it (from 1).
    pub number: u32,
    /// Cycles of the read.
    pub cycles: u32,
    /// True for an index (barcode) read.
    pub indexed: bool,
    /// True when the read is sequenced as the reverse complement.
    pub reverse_complement: bool,
}

/// What `RunInfo.xml` says.
#[derive(Debug, Clone, Default)]
pub struct RunInfo {
    /// `RunInfo@Version`.
    pub version: Option<String>,
    /// Run id (the run folder's name).
    pub run_id: String,
    /// Run number on the instrument.
    pub run_number: Option<u32>,
    /// Flow-cell id.
    pub flowcell: Option<String>,
    /// Instrument id (serial).
    pub instrument: Option<String>,
    /// Run date as written (`220422` or `6/24/2020 4:30:24 PM`).
    pub date: Option<String>,
    /// Reads in order.
    pub reads: Vec<ReadInfo>,
    /// Lanes.
    pub lane_count: u32,
    /// Surfaces per lane.
    pub surface_count: u32,
    /// Swaths per surface.
    pub swath_count: u32,
    /// Tiles per swath.
    pub tile_count: u32,
    /// Tile naming convention (`FourDigit`, `FiveDigit`).
    pub tile_naming: Option<String>,
    /// Tiles listed in the `TileSet` (`1_2101`), when it lists them.
    pub tiles: Vec<String>,
    /// Image channels (`RED`, `GREEN`; two-colour chemistry), when listed.
    pub image_channels: Vec<String>,
}

impl RunInfo {
    /// Total cycles over every read.
    pub fn total_cycles(&self) -> u32 {
        self.reads.iter().map(|r| r.cycles).sum()
    }

    /// `(read number, first cycle, last cycle)` per read, cycles numbered from 1 over the run.
    pub fn layout(&self) -> Vec<(u32, u32, u32)> {
        let mut first = 1u32;
        let mut out = Vec::new();
        for r in &self.reads {
            if r.cycles == 0 {
                continue;
            }
            let last = first.saturating_add(r.cycles - 1);
            out.push((r.number, first, last));
            first = last.saturating_add(1);
        }
        out
    }

    /// The read structure as the conversion tools write it (`151T8B8B151T`).
    pub fn read_structure(&self) -> String {
        self.reads
            .iter()
            .map(|r| format!("{}{}", r.cycles, if r.indexed { 'B' } else { 'T' }))
            .collect()
    }
}

fn attr<T: std::str::FromStr>(n: Node<'_, '_>, name: &str) -> Option<T> {
    n.attribute(name).and_then(|v| v.trim().parse().ok())
}

fn yes(n: Node<'_, '_>, name: &str) -> bool {
    n.attribute(name)
        .is_some_and(|v| matches!(v.trim(), "Y" | "y" | "true" | "True" | "1"))
}

fn child_text(n: Node<'_, '_>, name: &str) -> Option<String> {
    n.children()
        .find(|c| c.has_tag_name(name))
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Parse `RunInfo.xml`.
pub fn parse_runinfo(text: &str) -> Result<RunInfo> {
    let doc = Document::parse(text)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("RunInfo.xml is not XML: {e}")))?;
    let root = doc.root_element();
    if !root.has_tag_name("RunInfo") {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "RunInfo.xml has root element <{}>, expected <RunInfo>",
                root.tag_name().name()
            ),
        ));
    }
    let run = root
        .children()
        .find(|c| c.has_tag_name("Run"))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "RunInfo.xml has no <Run> element"))?;
    let mut ri = RunInfo {
        version: root.attribute("Version").map(str::to_string),
        run_id: run.attribute("Id").unwrap_or_default().to_string(),
        run_number: attr(run, "Number"),
        flowcell: child_text(run, "Flowcell"),
        instrument: child_text(run, "Instrument"),
        date: child_text(run, "Date"),
        ..RunInfo::default()
    };
    if let Some(reads) = run.children().find(|c| c.has_tag_name("Reads")) {
        for r in reads.children().filter(|c| c.has_tag_name("Read")) {
            ri.reads.push(ReadInfo {
                number: attr(r, "Number").unwrap_or(ri.reads.len() as u32 + 1),
                cycles: attr(r, "NumCycles").unwrap_or(0),
                indexed: yes(r, "IsIndexedRead"),
                reverse_complement: yes(r, "IsReverseComplement"),
            });
        }
    }
    if let Some(l) = run.children().find(|c| c.has_tag_name("FlowcellLayout")) {
        ri.lane_count = attr(l, "LaneCount").unwrap_or(0);
        ri.surface_count = attr(l, "SurfaceCount").unwrap_or(0);
        ri.swath_count = attr(l, "SwathCount").unwrap_or(0);
        ri.tile_count = attr(l, "TileCount").unwrap_or(0);
        if let Some(ts) = l.children().find(|c| c.has_tag_name("TileSet")) {
            ri.tile_naming = ts.attribute("TileNamingConvention").map(str::to_string);
            if let Some(tiles) = ts.children().find(|c| c.has_tag_name("Tiles")) {
                ri.tiles = tiles
                    .children()
                    .filter(|c| c.has_tag_name("Tile"))
                    .filter_map(|c| c.text().map(|t| t.trim().to_string()))
                    .collect();
            }
        }
    }
    if let Some(ch) = run.children().find(|c| c.has_tag_name("ImageChannels")) {
        ri.image_channels = ch
            .children()
            .filter(|c| c.has_tag_name("Name"))
            .filter_map(|c| c.text().map(|t| t.trim().to_string()))
            .collect();
    }
    if ri.reads.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "RunInfo.xml lists no reads"));
    }
    Ok(ri)
}

/// What `RunParameters.xml` (or MiSeq's `runParameters.xml`) says, as far as it is read.
#[derive(Debug, Clone, Default)]
pub struct RunParameters {
    /// Control software (`MiSeq Control Software`, `NovaSeq Control Software`).
    pub application: Option<String>,
    /// Its version.
    pub application_version: Option<String>,
    /// Real-time analysis version.
    pub rta_version: Option<String>,
    /// Experiment name the operator entered.
    pub experiment: Option<String>,
    /// Instrument type, when the file names it (`NextSeq 2000`, `NovaSeqXPlus`).
    pub instrument_type: Option<String>,
    /// Flow-cell mode or type (`SP`, `S4`, `P3`).
    pub flowcell_mode: Option<String>,
    /// Chemistry or reagent kit version.
    pub chemistry: Option<String>,
    /// Library tube or sample-loading barcode.
    pub library_tube: Option<String>,
    /// Run start date as written.
    pub run_start: Option<String>,
    /// Operator / user name.
    pub operator: Option<String>,
    /// Workflow or application type (`GenerateFASTQ`, `Resequencing`).
    pub workflow: Option<String>,
}

/// The first element anywhere in the document whose name is one of `names` (in order of
/// preference), case-insensitive, with non-empty text.
fn first_text(doc: &Document<'_>, names: &[&str]) -> Option<String> {
    for name in names {
        let hit = doc.descendants().find(|n| {
            n.is_element()
                && n.tag_name().name().eq_ignore_ascii_case(name)
                && n.text().is_some_and(|t| !t.trim().is_empty())
        });
        if let Some(n) = hit {
            return n.text().map(|t| t.trim().to_string());
        }
    }
    None
}

/// Parse `RunParameters.xml`. Unknown layouts give empty fields, not errors.
pub fn parse_runparameters(text: &str) -> Option<RunParameters> {
    let doc = Document::parse(text).ok()?;
    Some(RunParameters {
        application: first_text(&doc, &["ApplicationName", "Application"]),
        application_version: first_text(&doc, &["ApplicationVersion"]),
        rta_version: first_text(&doc, &["RtaVersion", "RTAVersion", "RTAVersionNumber"]),
        experiment: first_text(&doc, &["ExperimentName"]),
        instrument_type: first_text(&doc, &["InstrumentType"]),
        flowcell_mode: first_text(&doc, &["FlowCellMode", "FlowCellType", "Mode"]),
        chemistry: first_text(&doc, &["Chemistry", "ChemistryVersion", "ReagentKitVersion"]),
        library_tube: first_text(&doc, &["LibraryTubeSerialBarcode", "LibraryTubeID"]),
        run_start: first_text(&doc, &["RunStartDate", "RunStartTime"]),
        operator: first_text(&doc, &["Username", "UserName", "OperatorName"]),
        workflow: first_text(&doc, &["WorkflowType", "Workflow"]),
    })
}

/// The instrument model from the control software's name, else from the leading letters of
/// the instrument id (inferred: the convention seen across public run folders).
pub fn instrument_model(app: Option<&str>, kind: Option<&str>, id: Option<&str>) -> Option<(String, bool)> {
    if let Some(k) = kind.filter(|k| !k.is_empty()) {
        return Some((k.to_string(), true));
    }
    let by_id = id.and_then(model_of_id);
    if let Some(a) = app {
        let a = a.trim();
        for suffix in [" Control Software", " Control"] {
            if let Some(m) = a.strip_suffix(suffix) {
                let m = m.trim();
                // the software names the family (`NovaSeq`); the id prefix the model in it
                if let Some(full) = by_id.as_ref().filter(|f| f.starts_with(m) && f.len() > m.len()) {
                    return Some((full.clone(), false));
                }
                return Some((m.to_string(), true));
            }
        }
    }
    by_id.map(|m| (m, false))
}

/// The model an instrument id's leading letters stand for.
fn model_of_id(id: &str) -> Option<String> {
    let id = id.trim();
    let prefixes: &[(&str, &str)] = &[
        ("LH", "NovaSeq X"),
        ("VH", "NextSeq 1000/2000"),
        ("NB", "NextSeq 500/550"),
        ("NS", "NextSeq 500/550"),
        ("MN", "MiniSeq"),
        ("FS", "iSeq 100"),
        ("SH", "MiSeq i100"),
        ("M", "MiSeq"),
        ("A", "NovaSeq 6000"),
        ("E", "HiSeq X"),
        ("K", "HiSeq 3000/4000"),
        ("J", "HiSeq 3000/4000"),
        ("D", "HiSeq 2500"),
        ("SN", "HiSeq 2000"),
    ];
    prefixes
        .iter()
        .find(|(p, _)| {
            id.starts_with(p)
                && id[p.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
        })
        .map(|(_, m)| (*m).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runinfo_reads_and_layout() {
        let x = r#"<?xml version="1.0"?><RunInfo Version="5"><Run Id="R" Number="7">
          <Flowcell>F</Flowcell><Instrument>A00834</Instrument><Date>6/24/2020</Date>
          <Reads><Read Number="1" NumCycles="36" IsIndexedRead="N"/>
          <Read Number="2" NumCycles="10" IsIndexedRead="Y"/></Reads>
          <FlowcellLayout LaneCount="2" SurfaceCount="2" SwathCount="2" TileCount="78">
          <TileSet TileNamingConvention="FourDigit"><Tiles><Tile>1_2101</Tile></Tiles></TileSet>
          </FlowcellLayout></Run></RunInfo>"#;
        let r = parse_runinfo(x).unwrap();
        assert_eq!(r.layout(), vec![(1, 1, 36), (2, 37, 46)]);
        assert_eq!(r.read_structure(), "36T10B");
        assert_eq!(r.tiles, vec!["1_2101"]);
        assert_eq!(
            instrument_model(None, None, r.instrument.as_deref()),
            Some(("NovaSeq 6000".into(), false))
        );
        assert!(parse_runinfo("<RunInfo/>").is_err());
        assert!(parse_runinfo("not xml").is_err());
    }
}
