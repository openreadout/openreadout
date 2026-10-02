//! OpenLab CDS XML parts: the injection manifest (`injection.acmd` inside a `.dx`), the
//! injection results (`Base/InjectionACAML` inside a `.rx`) and the sequence file (`.acaml` in a
//! result-set folder). Elements are matched by local name (the namespaces are
//! `urn:schemas-agilent-com:acmd20` and `urn:schemas-agilent-com:acaml21` in the corpus).
//! Layout and derivation: `docs/formats/openlab-cds.md`, `docs/provenance/openlab-cds.md`.

use openreadout_core::xml::{child, children};
use roxmltree::{Document, Node, ParsingOptions};

/// Largest XML part parsed (sequence files of 100 injections are ~1.3 MB).
pub const MAX_XML_BYTES: usize = 64 << 20;

/// One `Signal` of the injection manifest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManifestSignal {
    /// Content type of the part (`Agilent.OpenLab.Rawdata/Signal179`, `…/InstrumentTrace179`).
    pub encoding: String,
    /// GUID naming the part (`<trace_id>.CH`, `<trace_id>.IT`).
    pub trace_id: String,
    /// Module abbreviation (`FID`, `DAD`, `PMP`).
    pub device: Option<String>,
    pub device_number: Option<u32>,
    /// `FID2B`, `DAD1D`, `PMP1A`.
    pub channel: String,
    /// `DAD1D,Sig=228,4 Ref=360,100`, `PMP1A,Pressure`.
    pub description: Option<String>,
    pub time_start: Option<f64>,
    pub time_end: Option<f64>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    /// Scale factor (equals the part header's scale).
    pub slope: Option<f64>,
    pub declared_values: Option<u64>,
    /// Spectra parts: the number of spectra (`NumberOfRecords`).
    pub declared_records: Option<u64>,
    pub units: Option<String>,
    /// `true` for detector signals, `false` for instrument curves.
    pub integrable: Option<bool>,
}

impl ManifestSignal {
    /// The part's short content type (`Signal179`, `InstrumentTrace179`).
    pub fn kind(&self) -> &str {
        self.encoding.rsplit('/').next().unwrap_or(&self.encoding)
    }
}

/// The injection manifest (`injection.acmd`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InjectionManifest {
    pub version: Option<String>,
    /// Autosampler position as written (`71`).
    pub location: Option<String>,
    pub injection_source: Option<String>,
    pub injection_volume: Option<f64>,
    pub injection_volume_unit: Option<String>,
    pub sequence_line: Option<u32>,
    pub replicate: Option<u32>,
    pub sample_name: Option<String>,
    pub operator: Option<String>,
    pub barcode: Option<String>,
    /// Start of the run, ISO-8601 as written.
    pub run_started: Option<String>,
    /// Acquisition method path as written.
    pub acquisition_method: Option<String>,
    pub signals: Vec<ManifestSignal>,
}

/// One integrated peak of the vendor results.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VendorPeak {
    pub peak_id: String,
    pub rt_min: f64,
    /// `NormalPeak`, ...
    pub peak_type: Option<String>,
    pub area: Option<f64>,
    /// `pA·s`, `mAU·s`.
    pub area_unit: Option<String>,
    pub area_percent: Option<f64>,
    pub height: Option<f64>,
    pub height_unit: Option<String>,
    pub height_percent: Option<f64>,
    pub symmetry: Option<f64>,
    pub start_min: Option<f64>,
    pub end_min: Option<f64>,
    /// `BB`, `BV`, ... (trailing blanks removed).
    pub baseline_code: Option<String>,
    /// Baseline value at the start and end of the peak (signal units).
    pub baseline_start: Option<f64>,
    pub baseline_end: Option<f64>,
    pub width_base_min: Option<f64>,
    /// `Linear`, ...
    pub baseline_model: Option<String>,
    /// Baseline value at the retention time.
    pub baseline_at_apex: Option<f64>,
    /// Compound assigned to the peak (empty names are `None`).
    pub compound: Option<String>,
    /// `Unknown`, ...
    pub compound_type: Option<String>,
    /// Expected retention time of the compound (finite values only).
    pub expected_rt_min: Option<f64>,
}

/// Peaks of one signal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignalResults {
    /// Id of the signal in the sequence file (not a manifest `trace_id`).
    pub signal_id: String,
    pub peaks: Vec<VendorPeak>,
}

/// The injection results of a `.rx` package.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InjectionResults {
    /// Id of the injection's measurement in the sequence file.
    pub measurement_id: Option<String>,
    pub processing_method: Option<String>,
    pub processing_method_version: Option<String>,
    /// `ESTD`, ...
    pub quantitation: Option<String>,
    /// Data-analysis software as written.
    pub software: Option<String>,
    pub integrator: Option<String>,
    /// `Passed`, ...
    pub processing_state: Option<String>,
    /// When the results document was written.
    pub processed_at: Option<String>,
    pub signals: Vec<SignalResults>,
}

/// One module of the instrument (sequence file).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstrumentModule {
    pub id: String,
    pub name: Option<String>,
    pub manufacturer: Option<String>,
    /// `Pump`, `Sampler`, `ColumnCompartment`, `Detector`.
    pub kind: Option<String>,
    pub part_number: Option<String>,
    pub serial_number: Option<String>,
    pub firmware: Option<String>,
}

/// One signal of an injection in the sequence file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SequenceSignal {
    /// The id `.rx` results refer to.
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// `InstrumentCurve`, ...
    pub kind: Option<String>,
    /// The manifest `trace_id`.
    pub trace_id: Option<String>,
    pub detector: Option<String>,
    pub module_id: Option<String>,
}

/// One injection of the sequence file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SequenceInjection {
    pub measurement_id: String,
    /// The `.dx` file name.
    pub data_file: Option<String>,
    pub signals: Vec<SequenceSignal>,
}

/// The sequence file (`<result set>.acaml`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SequenceFile {
    pub instrument_name: Option<String>,
    /// `LiquidChromatography`, `GasChromatography`.
    pub technique: Option<String>,
    pub modules: Vec<InstrumentModule>,
    /// Acquisition software (name and version) of the first injection.
    pub acquisition_software: Option<String>,
    pub injections: Vec<SequenceInjection>,
}

impl SequenceFile {
    /// The injection whose data file is `dx_name` (case-insensitive) or whose measurement id
    /// is `measurement_id`.
    pub fn injection(
        &self,
        dx_name: &str,
        measurement_id: Option<&str>,
    ) -> Option<&SequenceInjection> {
        self.injections
            .iter()
            .find(|i| {
                i.data_file
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case(dx_name))
            })
            .or_else(|| {
                measurement_id.and_then(|m| self.injections.iter().find(|i| i.measurement_id == m))
            })
    }
}

fn parse(xml: &str) -> Result<Document<'_>, String> {
    if xml.len() > MAX_XML_BYTES {
        return Err(format!(
            "{} bytes of XML (limit {MAX_XML_BYTES})",
            xml.len()
        ));
    }
    let opts = ParsingOptions {
        allow_dtd: false,
        nodes_limit: 5_000_000,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(xml, opts).map_err(|e| format!("XML: {e}"))
}

fn text(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn text_num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    text(n, name).and_then(|s| number(&s))
}

fn text_u32(n: Node<'_, '_>, name: &str) -> Option<u32> {
    text(n, name).and_then(|s| s.parse().ok())
}

/// `<Name val="…" unit="…"/>`: the value (finite) and the unit (non-empty).
fn val(n: Node<'_, '_>, name: &str) -> (Option<f64>, Option<String>) {
    match child(n, name) {
        Some(c) => (
            c.attribute("val").and_then(number),
            c.attribute("unit")
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string),
        ),
        None => (None, None),
    }
}

/// A time value in minutes (`unit` `min`, `s`, `ms`; no unit = minutes).
fn minutes(n: Node<'_, '_>, name: &str) -> Option<f64> {
    let (v, u) = val(n, name);
    let v = v?;
    match u.as_deref() {
        None | Some("min") => Some(v),
        Some("s") => Some(v / 60.0),
        Some("ms") => Some(v / 60_000.0),
        Some("h") => Some(v * 60.0),
        Some(_) => None,
    }
}

/// Parse `injection.acmd`.
pub fn parse_manifest(xml: &str) -> Result<InjectionManifest, String> {
    let doc = parse(xml)?;
    let root = doc.root_element();
    if root.tag_name().name() != "ACMD" {
        return Err(format!(
            "root element is <{}>, not <ACMD>",
            root.tag_name().name()
        ));
    }
    let info = child(root, "InjectionInfo").ok_or("no <InjectionInfo> element")?;
    let mut m = InjectionManifest {
        version: text(info, "Version"),
        location: text(info, "Location"),
        injection_source: text(info, "InjectionSource"),
        injection_volume: text_num(info, "InjectionVolume"),
        injection_volume_unit: text(info, "InjectionVolumeUnits"),
        sequence_line: text_u32(info, "SequenceLine"),
        replicate: text_u32(info, "Replicate"),
        sample_name: text(info, "SampleName"),
        operator: text(info, "RunOperator"),
        barcode: text(info, "Barcode"),
        run_started: text(info, "RunDateTime"),
        acquisition_method: text(info, "AcquisitionMethod"),
        signals: Vec::new(),
    };
    if let Some(sigs) = child(info, "Signals") {
        for s in children(sigs, "Signal") {
            let Some(trace_id) = text(s, "TraceId") else {
                continue;
            };
            m.signals.push(ManifestSignal {
                encoding: text(s, "Encoding").unwrap_or_default(),
                channel: text(s, "ChannelName").unwrap_or_else(|| trace_id.clone()),
                trace_id,
                device: text(s, "DeviceName"),
                device_number: text_u32(s, "DeviceNumber"),
                description: text(s, "Description"),
                time_start: text_num(s, "TimeStart"),
                time_end: text_num(s, "TimeEnd"),
                minimum: text_num(s, "Minimum"),
                maximum: text_num(s, "Maximum"),
                slope: text_num(s, "Slope"),
                declared_values: text(s, "NumberOfValues").and_then(|v| v.parse().ok()),
                declared_records: text(s, "NumberOfRecords").and_then(|v| v.parse().ok()),
                units: text(s, "Units"),
                integrable: text(s, "IsIntegrable").map(|v| v.eq_ignore_ascii_case("true")),
            });
        }
    }
    Ok(m)
}

fn app(n: Node<'_, '_>) -> Option<String> {
    let a = child(n, "AgilentApp")?;
    match (text(a, "Name"), text(a, "Version")) {
        (Some(n), Some(v)) => Some(format!("{n} {v}")),
        (n, v) => n.or(v),
    }
}

/// Parse the results document of a `.rx` package (`Base/InjectionACAML`).
pub fn parse_results(xml: &str) -> Result<InjectionResults, String> {
    let doc = parse(xml)?;
    let root = doc.root_element();
    if root.tag_name().name() != "ACAML" {
        return Err(format!(
            "root element is <{}>, not <ACAML>",
            root.tag_name().name()
        ));
    }
    let d = child(root, "Doc").ok_or("no <Doc> element")?;
    let content = child(d, "Content").ok_or("no <Doc>/<Content> element")?;
    let mut r = InjectionResults {
        processed_at: child(d, "DocInfo").and_then(|i| text(i, "CreationDate")),
        ..InjectionResults::default()
    };
    if let Some(m) = child(content, "Method") {
        r.processing_method = text(m, "Name");
        r.processing_method_version = text(m, "OriginalVersion");
        r.quantitation = text(m, "QuantitationMethod");
    }
    let Some(inj) = child(content, "Injections") else {
        return Ok(r);
    };
    for res in children(inj, "Result") {
        if r.measurement_id.is_none() {
            r.measurement_id = child(res, "InjectionMeasData_ID")
                .and_then(|c| c.attribute("id"))
                .map(str::to_string);
        }
        r.software = r
            .software
            .take()
            .or_else(|| text(res, "DataAnalysisSoftware"))
            .or_else(|| child(res, "DataAnalysisApplication").and_then(app));
        r.integrator = r.integrator.take().or_else(|| text(res, "Integrator"));
        if r.processing_state.is_none() {
            r.processing_state =
                child(res, "ProcessingStatus").and_then(|p| text(p, "TransformationChainState"));
        }
        if r.processing_method_version.is_none() {
            r.processing_method_version = text(res, "DataAnalysisMethodVersion");
        }
        // compounds, by the peak they identify
        let mut compounds: Vec<(String, Node<'_, '_>)> = Vec::new();
        for c in children(res, "InjectionCompound") {
            let ids = child(c, "Identification")
                .and_then(|i| child(i, "Qualified"))
                .and_then(|q| child(q, "Peaks"))
                .map(|p| {
                    children(p, "Peak_ID")
                        .filter_map(|x| x.attribute("id").map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for id in ids {
                compounds.push((id, c));
            }
        }
        for sr in children(res, "SignalResult") {
            let signal_id = child(sr, "Signal_ID")
                .and_then(|c| c.attribute("id"))
                .unwrap_or_default()
                .to_string();
            let mut peaks = Vec::new();
            for p in children(sr, "Peak") {
                let Some(rt) = minutes(p, "RetentionTime") else {
                    continue;
                };
                let id = p.attribute("id").unwrap_or_default().to_string();
                let (area, area_unit) = val(p, "Area");
                let (height, height_unit) = val(p, "Height");
                let comp = compounds.iter().find(|(pid, _)| *pid == id).map(|x| x.1);
                peaks.push(VendorPeak {
                    rt_min: rt,
                    peak_type: text(p, "Type"),
                    area,
                    area_unit,
                    area_percent: val(p, "AreaPercent").0,
                    height,
                    height_unit,
                    height_percent: val(p, "HeightPercent").0,
                    symmetry: val(p, "Symmetry").0,
                    start_min: minutes(p, "BeginTime"),
                    end_min: minutes(p, "EndTime"),
                    baseline_code: text(p, "BaselineCode"),
                    baseline_start: val(p, "BaselineStart").0,
                    baseline_end: val(p, "BaselineEnd").0,
                    width_base_min: minutes(p, "WidthBase"),
                    baseline_model: text(p, "BaselineModel"),
                    baseline_at_apex: val(p, "BaselineRetentionHeight").0,
                    compound: comp.and_then(|c| text(c, "CompoundName")),
                    compound_type: comp.and_then(|c| text(c, "Type")),
                    expected_rt_min: comp.and_then(|c| minutes(c, "ExpectedRetTime")),
                    peak_id: id,
                });
            }
            r.signals.push(SignalResults { signal_id, peaks });
        }
    }
    Ok(r)
}

/// Parse a sequence file (`<result set>.acaml`).
pub fn parse_sequence(xml: &str) -> Result<SequenceFile, String> {
    let doc = parse(xml)?;
    let root = doc.root_element();
    if root.tag_name().name() != "ACAML" {
        return Err(format!(
            "root element is <{}>, not <ACAML>",
            root.tag_name().name()
        ));
    }
    let content = child(root, "Doc")
        .and_then(|d| child(d, "Content"))
        .ok_or("no <Doc>/<Content> element")?;
    let mut s = SequenceFile::default();
    if let Some(ins) = child(content, "Resources").and_then(|r| child(r, "Instrument")) {
        s.instrument_name = text(ins, "Name");
        s.technique = text(ins, "Technique");
        for m in children(ins, "Module") {
            s.modules.push(InstrumentModule {
                id: m.attribute("id").unwrap_or_default().to_string(),
                name: text(m, "Name"),
                manufacturer: text(m, "Manufacturer"),
                kind: text(m, "Type"),
                part_number: text(m, "PartNo"),
                serial_number: text(m, "SerialNo"),
                firmware: text(m, "FirmwareRevision"),
            });
        }
    }
    if let Some(inj) = child(content, "Injections") {
        for md in children(inj, "MeasData") {
            if s.acquisition_software.is_none() {
                s.acquisition_software = child(md, "AcquisitionApplication")
                    .and_then(app)
                    .or_else(|| text(md, "AcquisitionSoftware"));
            }
            let data_file = child(md, "BinaryData")
                .and_then(|b| child(b, "DataItem"))
                .and_then(|i| text(i, "Name"));
            let signals = children(md, "Signal")
                .map(|g| SequenceSignal {
                    id: g.attribute("id").unwrap_or_default().to_string(),
                    name: text(g, "Name"),
                    description: text(g, "Description"),
                    kind: text(g, "Type"),
                    trace_id: text(g, "TraceID"),
                    detector: text(g, "DetectorName"),
                    module_id: child(g, "InstrumentModule_ID")
                        .and_then(|c| c.attribute("id"))
                        .map(str::to_string),
                })
                .collect();
            s.injections.push(SequenceInjection {
                measurement_id: md.attribute("id").unwrap_or_default().to_string(),
                data_file,
                signals,
            });
        }
    }
    Ok(s)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const ACMD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<ACMD xmlns="urn:schemas-agilent-com:acmd20"><InjectionInfo><Version>2</Version>
<Location>71</Location><InjectionSource>GC Injector</InjectionSource>
<InjectionVolume>1</InjectionVolume><InjectionVolumeUnits>μL</InjectionVolumeUnits>
<SequenceLine>2</SequenceLine><Replicate>1</Replicate><SampleName>S1</SampleName>
<RunOperator>Admin</RunOperator><Barcode></Barcode>
<RunDateTime>2023-02-23T15:05:20.4882635+01:00</RunDateTime>
<AcquisitionMethod>D:\x\m.amx</AcquisitionMethod><Signals><Signal>
<Encoding>Agilent.OpenLab.Rawdata/Signal179</Encoding><TraceId>abc</TraceId>
<DeviceName>FID</DeviceName><DeviceNumber>1</DeviceNumber><ChannelName>FID2B</ChannelName>
<Description>FID2B</Description><TimeStart>19.8</TimeStart><TimeEnd>680000</TimeEnd>
<Slope>0.5</Slope><NumberOfValues>34000</NumberOfValues><Units>pA</Units>
<IsIntegrable>true</IsIntegrable></Signal><Signal><Encoding>x</Encoding></Signal></Signals>
</InjectionInfo></ACMD>"#;

    #[test]
    fn manifest() {
        let m = parse_manifest(ACMD).unwrap();
        assert_eq!(m.location.as_deref(), Some("71"));
        assert_eq!(m.injection_volume, Some(1.0));
        assert_eq!(m.barcode, None);
        assert_eq!(m.signals.len(), 1, "a signal without TraceId is skipped");
        let s = &m.signals[0];
        assert_eq!(s.kind(), "Signal179");
        assert_eq!(s.channel, "FID2B");
        assert_eq!(s.declared_values, Some(34_000));
        assert_eq!(s.integrable, Some(true));
        assert!(parse_manifest("<ACAML/>").is_err());
        assert!(parse_manifest("<ACMD/>").is_err());
        assert!(parse_manifest("<ACMD><InjectionInfo>").is_err());
        assert!(parse_manifest("<!DOCTYPE x [<!ENTITY a 'b'>]><ACMD/>").is_err());
    }

    #[test]
    fn results() {
        let xml = r#"<ACAML xmlns="urn:schemas-agilent-com:acaml21"><Doc><DocInfo>
<CreationDate>2023-02-23T15:36:17+01:00</CreationDate></DocInfo><Content>
<Method><Name>default_integration</Name><QuantitationMethod>ESTD</QuantitationMethod></Method>
<Injections><Result><InjectionMeasData_ID id="m1"/>
<SignalResult><Signal_ID id="s0"/></SignalResult>
<SignalResult><Signal_ID id="s1"/>
<Peak id="p1"><RetentionTime val="7.2" unit="min"/><Type>NormalPeak</Type>
<Area val="5.13" unit="pA·s"/><AreaPercent val="0.2"/><Height val="3.3" unit="pA"/>
<BeginTime val="429" unit="s"/><BaselineCode>BB  </BaselineCode><WidthBase val="0.13"/></Peak>
<Peak id="p2"><RetentionTime val="NaN" unit="min"/></Peak>
</SignalResult>
<InjectionCompound><CompoundName>caffeine</CompoundName><Identification><Qualified><Peaks>
<Peak_ID id="p1"/></Peaks></Qualified></Identification><Type>Target</Type>
<ExpectedRetTime val="-INF" unit="min"/></InjectionCompound>
<DataAnalysisSoftware>OpenLAB 2.6</DataAnalysisSoftware><Integrator>TwelveTone</Integrator>
</Result></Injections></Content></Doc></ACAML>"#;
        let r = parse_results(xml).unwrap();
        assert_eq!(r.measurement_id.as_deref(), Some("m1"));
        assert_eq!(r.quantitation.as_deref(), Some("ESTD"));
        assert_eq!(r.signals.len(), 2);
        assert!(r.signals[0].peaks.is_empty());
        let p = &r.signals[1].peaks;
        assert_eq!(
            p.len(),
            1,
            "a peak without a finite retention time is skipped"
        );
        assert_eq!(p[0].area_unit.as_deref(), Some("pA·s"));
        assert_eq!(p[0].start_min, Some(429.0 / 60.0));
        assert_eq!(p[0].baseline_code.as_deref(), Some("BB"));
        assert_eq!(p[0].compound.as_deref(), Some("caffeine"));
        assert_eq!(p[0].expected_rt_min, None);
        assert_eq!(r.software.as_deref(), Some("OpenLAB 2.6"));
    }

    #[test]
    fn sequence() {
        let xml = r#"<ACAML><Doc><Content><Resources><Instrument><Name>Luxo HPLC</Name>
<Technique>LiquidChromatography</Technique><Module id="m1"><Name>DAD</Name><Type>Detector</Type>
<PartNo>G7117C</PartNo><SerialNo>DE1</SerialNo></Module></Instrument></Resources>
<Injections><MeasData id="i1"><BinaryData><DataItem><Name>a.dx</Name></DataItem></BinaryData>
<Signal id="g1"><Name>DAD1D</Name><TraceID>t1</TraceID><InstrumentModule_ID id="m1"/></Signal>
<AcquisitionApplication><AgilentApp><Name>Acq</Name><Version>2.5</Version></AgilentApp></AcquisitionApplication>
</MeasData></Injections></Content></Doc></ACAML>"#;
        let s = parse_sequence(xml).unwrap();
        assert_eq!(s.modules[0].part_number.as_deref(), Some("G7117C"));
        assert_eq!(s.acquisition_software.as_deref(), Some("Acq 2.5"));
        let i = s.injection("A.DX", None).unwrap();
        assert_eq!(i.signals[0].trace_id.as_deref(), Some("t1"));
        assert!(s.injection("b.dx", Some("i1")).is_some());
        assert!(s.injection("b.dx", Some("zz")).is_none());
    }
}
