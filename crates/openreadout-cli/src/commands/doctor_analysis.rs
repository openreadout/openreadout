//! The analysis half of `doctor`'s self-test: small synthetic inputs written in memory for the
//! analysis readers and commands (mzML + `chromatogram` + `peaks`, RDML + `qpcr`, a long-form
//! plate CSV + `assay-curve`, Gating-ML + `gate`, JCAMP-DX spectra, and a `batch` table over the
//! other fixtures). Every expected value is computed here from what was written, never from the
//! reader. No corpus file is embedded or needed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Error, Registry, Result};
use serde_json::{Value, json};

use super::doctor::{DoctorCheck, check, expect};

// ---------- mzML: 31 MS1 scans, one Gaussian elution profile at m/z 250 ----------

const SCANS: usize = 31;
const SCAN_STEP_MIN: f64 = 0.1;
const MZS: [f64; 3] = [150.0, 250.0, 350.0];
const APEX_RT_MIN: f64 = 1.5;
const SIGMA_MIN: f64 = 0.2;
const APEX_HEIGHT: f64 = 10_000.0;
const FLAT: f64 = 100.0;

fn scan_rt(k: usize) -> f64 {
    k as f64 * SCAN_STEP_MIN
}

/// Intensity at m/z 250 in scan `k` (whole numbers, exact in f32), on top of a small ripple so
/// the noise estimate is not zero.
fn xic_value(k: usize) -> f64 {
    let d = (scan_rt(k) - APEX_RT_MIN) / SIGMA_MIN;
    (APEX_HEIGHT * (-0.5 * d * d).exp()).round() + [3.0, 5.0, 4.0][k % 3]
}

fn scan_intensities(k: usize) -> [f64; 3] {
    [FLAT, xic_value(k), FLAT + (k % 2) as f64]
}

/// Trapezoidal area of the m/z 250 profile above its lowest ripple, intensity × minutes.
fn expected_xic_area() -> f64 {
    let y: Vec<f64> = (0..SCANS).map(|k| xic_value(k) - 4.0).collect();
    y.windows(2)
        .map(|w| f64::midpoint(w[0], w[1]) * SCAN_STEP_MIN)
        .sum()
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(char::from(T[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// A plain (non-indexed) mzML 1.1 run of [`SCANS`] centroided MS1 scans.
pub fn mzml_fixture() -> String {
    let mut spectra = String::new();
    for k in 0..SCANS {
        let mz: Vec<u8> = MZS.iter().flat_map(|v| v.to_le_bytes()).collect();
        let it: Vec<u8> = scan_intensities(k)
            .iter()
            .flat_map(|v| (*v as f32).to_le_bytes())
            .collect();
        let (m, i) = (base64(&mz), base64(&it));
        spectra.push_str(&format!(
            r#"      <spectrum index="{k}" id="scan={n}" defaultArrayLength="3">
        <cvParam cvRef="MS" accession="MS:1000511" name="ms level" value="1"/>
        <cvParam cvRef="MS" accession="MS:1000579" name="MS1 spectrum" value=""/>
        <cvParam cvRef="MS" accession="MS:1000127" name="centroid spectrum" value=""/>
        <cvParam cvRef="MS" accession="MS:1000130" name="positive scan" value=""/>
        <scanList count="1">
          <cvParam cvRef="MS" accession="MS:1000795" name="no combination" value=""/>
          <scan><cvParam cvRef="MS" accession="MS:1000016" name="scan start time" value="{rt}" unitCvRef="UO" unitAccession="UO:0000031" unitName="minute"/></scan>
        </scanList>
        <binaryDataArrayList count="2">
          <binaryDataArray encodedLength="{ml}">
            <cvParam cvRef="MS" accession="MS:1000523" name="64-bit float" value=""/>
            <cvParam cvRef="MS" accession="MS:1000576" name="no compression" value=""/>
            <cvParam cvRef="MS" accession="MS:1000514" name="m/z array" value="" unitCvRef="MS" unitAccession="MS:1000040" unitName="m/z"/>
            <binary>{m}</binary>
          </binaryDataArray>
          <binaryDataArray encodedLength="{il}">
            <cvParam cvRef="MS" accession="MS:1000521" name="32-bit float" value=""/>
            <cvParam cvRef="MS" accession="MS:1000576" name="no compression" value=""/>
            <cvParam cvRef="MS" accession="MS:1000515" name="intensity array" value="" unitCvRef="MS" unitAccession="MS:1000131" unitName="number of detector counts"/>
            <binary>{i}</binary>
          </binaryDataArray>
        </binaryDataArrayList>
      </spectrum>
"#,
            n = k + 1,
            rt = scan_rt(k),
            ml = m.len(),
            il = i.len(),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<mzML xmlns="http://psi.hupo.org/ms/mzml" id="doctor" version="1.1.0">
  <cvList count="2">
    <cv id="MS" fullName="Proteomics Standards Initiative Mass Spectrometry Ontology" URI="https://raw.githubusercontent.com/HUPO-PSI/psi-ms-CV/master/psi-ms.obo"/>
    <cv id="UO" fullName="Unit Ontology" URI="https://raw.githubusercontent.com/bio-ontology-research-group/unit-ontology/master/unit.obo"/>
  </cvList>
  <fileDescription><fileContent><cvParam cvRef="MS" accession="MS:1000579" name="MS1 spectrum" value=""/></fileContent></fileDescription>
  <softwareList count="1"><software id="doctor" version="0"/></softwareList>
  <instrumentConfigurationList count="1"><instrumentConfiguration id="IC1"/></instrumentConfigurationList>
  <dataProcessingList count="1"><dataProcessing id="DP1"><processingMethod order="0" softwareRef="doctor"/></dataProcessing></dataProcessingList>
  <run id="doctor" defaultInstrumentConfigurationRef="IC1">
    <spectrumList count="{SCANS}" defaultDataProcessingRef="DP1">
{spectra}    </spectrumList>
  </run>
</mzML>
"#
    )
}

// ---------- RDML: one amplified well with a stored Cq, one no-template control ----------

const RDML_CQ: f64 = 21.5;
/// Half the plateau: the curve reaches it at the sigmoid's midpoint, cycle 22 (with or without
/// subtracting the flat baseline of about 1).
const RDML_THRESHOLD: f64 = 500.0;
const RDML_THRESHOLD_CYCLE: f64 = 22.0;

fn rdml_fluor(cycle: u32) -> f64 {
    1.0 + 1000.0 / (1.0 + (-(f64::from(cycle) - 22.0)).exp())
}

/// RDML 1.3 as bare XML.
pub fn rdml_fixture() -> String {
    let adp = (1..=40).fold(String::new(), |mut acc, c| {
        let _ = write!(
            acc,
            "<adp><cyc>{c}</cyc><fluor>{:.6}</fluor></adp>",
            rdml_fluor(c)
        );
        acc
    });
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            "\n",
            r#"<rdml xmlns="http://www.rdml.org" version="1.3"><dye id="SYBR"/>"#,
            r#"<sample id="S1"><type>unkn</type></sample><sample id="N"><type>ntc</type></sample>"#,
            r#"<target id="GAPDH"><type>ref</type><dyeId id="SYBR"/></target>"#,
            r#"<experiment id="E"><run id="R"><pcrFormat><rows>8</rows><columns>12</columns>"#,
            r#"<rowLabel>ABC</rowLabel><columnLabel>123</columnLabel></pcrFormat>"#,
            r#"<react id="1"><sample id="S1"/><data><tar id="GAPDH"/><cq>{cq}</cq>{adp}</data></react>"#,
            r#"<react id="2"><sample id="N"/><data><tar id="GAPDH"/><cq>-1</cq></data></react>"#,
            "</run></experiment></rdml>\n"
        ),
        cq = RDML_CQ,
        adp = adp
    )
}

// ---------- long-form plate CSV: a linear standard curve and one unknown ----------

const SLOPE: f64 = 0.02;
const INTERCEPT: f64 = 0.05;
const STANDARDS: [f64; 6] = [0.0, 10.0, 20.0, 40.0, 80.0, 160.0];
const UNKNOWN_CONC: f64 = 37.5;

fn response(conc: f64) -> f64 {
    SLOPE * conc + INTERCEPT
}

/// `well,value,role,sample,concentration` with standards in duplicate (rows A and B) and one
/// unknown in triplicate (C1–C3).
pub fn plate_fixture() -> String {
    let mut s = String::from("well,value,role,sample,concentration\n");
    for (i, c) in STANDARDS.iter().enumerate() {
        for row in ['A', 'B'] {
            s.push_str(&format!(
                "{row}{},{:.6},standard,STD{},{c}\n",
                i + 1,
                response(*c),
                i + 1
            ));
        }
    }
    for col in 1..=3 {
        s.push_str(&format!(
            "C{col},{:.6},sample,U1,\n",
            response(UNKNOWN_CONC)
        ));
    }
    s
}

// ---------- Gating-ML: a rectangle on the FCS fixture ----------

/// FSC-A in [20, 60): the FCS fixture's events 2..=5 (FSC-A = 10·e + 0.5).
pub fn gatingml_fixture() -> String {
    concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        "\n",
        r#"<gating:Gating-ML xmlns:gating="http://www.isac-net.org/std/Gating-ML/v2.0/gating" "#,
        r#"xmlns:data-type="http://www.isac-net.org/std/Gating-ML/v2.0/datatypes">"#,
        r#"<gating:RectangleGate gating:id="Box">"#,
        r#"<gating:dimension gating:min="20" gating:max="60" gating:compensation-ref="uncompensated">"#,
        r#"<data-type:fcs-dimension data-type:name="FSC-A"/></gating:dimension>"#,
        "</gating:RectangleGate></gating:Gating-ML>\n"
    )
    .to_string()
}

const GATE_COUNT: u64 = 4;

// ---------- JCAMP-DX: an IR absorbance spectrum ----------

const JDX_POINTS: usize = 64;

fn jdx_y(i: usize) -> i64 {
    let d = (i as f64 - 30.0) / 4.0;
    (1000.0 * (-d * d).exp()).round() as i64 + 10
}

/// JCAMP-DX 4.24 `(X++(Y..Y))`, x 400…1030 cm⁻¹, y = integer × 0.001.
pub fn jcamp_fixture() -> String {
    let mut s = format!(
        "##TITLE=openreadout doctor\n##JCAMP-DX=4.24\n##DATA TYPE=INFRARED SPECTRUM\n\
         ##ORIGIN=synthetic\n##OWNER=public domain\n##XUNITS=1/CM\n##YUNITS=ABSORBANCE\n\
         ##XFACTOR=1\n##YFACTOR=0.001\n##FIRSTX=400\n##LASTX={}\n##DELTAX=10\n##NPOINTS={JDX_POINTS}\n\
         ##FIRSTY={}\n##XYDATA=(X++(Y..Y))\n",
        400 + (JDX_POINTS - 1) * 10,
        jdx_y(0) as f64 * 0.001
    );
    for i in (0..JDX_POINTS).step_by(8) {
        s.push_str(&(400 + i * 10).to_string());
        for j in i..(i + 8).min(JDX_POINTS) {
            s.push_str(&format!(" {}", jdx_y(j)));
        }
        s.push('\n');
    }
    s.push_str("##END=\n");
    s
}

/// The files the analysis checks read, written into `dir`.
pub struct Fixtures {
    pub mzml: PathBuf,
    pub rdml: PathBuf,
    pub plate: PathBuf,
    pub gatingml: PathBuf,
    pub jcamp: PathBuf,
}

pub fn write_fixtures(dir: &Path) -> Result<Fixtures> {
    let f = Fixtures {
        mzml: dir.join("doctor.mzML"),
        rdml: dir.join("doctor.rdml"),
        plate: dir.join("doctor-plate.csv"),
        gatingml: dir.join("doctor-gates.xml"),
        jcamp: dir.join("doctor.jdx"),
    };
    for (p, text) in [
        (&f.mzml, mzml_fixture()),
        (&f.rdml, rdml_fixture()),
        (&f.plate, plate_fixture()),
        (&f.gatingml, gatingml_fixture()),
        (&f.jcamp, jcamp_fixture()),
    ] {
        std::fs::write(p, text).map_err(|e| Error::io(p, e))?;
    }
    Ok(f)
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Other(format!("serialize: {e}")))
}

fn num(v: &Value, what: &str) -> Result<f64> {
    v.as_f64()
        .ok_or_else(|| Error::Other(format!("{what} missing in the output")))
}

fn close(got: f64, want: f64, rel: f64) -> bool {
    (got - want).abs() <= rel * want.abs().max(1e-12)
}

/// The analysis checks; `tif` and `fcs` are the base self-test files in the same directory.
pub fn checks(reg: &Registry, dir: &Path, tif: &Path, fcs: &Path) -> Vec<DoctorCheck> {
    let fx = match write_fixtures(dir) {
        Ok(fx) => fx,
        Err(e) => {
            return vec![DoctorCheck {
                name: "write analysis fixtures".into(),
                ok: false,
                detail: e.to_string(),
            }];
        }
    };
    let mut out = Vec::new();
    out.push(check("mzml spectra", || {
        let (_, d) = reg.detect(&fx.mzml)?;
        expect(d.format_id == "mzml", format!("detected {}", d.format_id))?;
        let (_, mut ds) = reg.open(&fx.mzml)?;
        let spec = to_json(&ds.read_spectrum(0, 15)?)?;
        let n = spec["mz"].as_array().map_or(0, Vec::len);
        expect(n == MZS.len(), format!("{n} points in scan 16"))?;
        let r = ds.check()?;
        expect(r.ok, format!("check: {} findings", r.findings.len()))?;
        Ok(format!("{SCANS} scans, scan 16 has {n} points; check ok"))
    }));
    out.push(check("chromatogram and peaks", || {
        let (_, mut ds) = reg.open(&fx.mzml)?;
        let info = ds.info()?;
        let ctx = ReadContext::default();
        let q: openreadout_quant::api::ChromatogramQuery =
            serde_json::from_value(json!({"mz": [250.0], "ppm": 10.0, "max_points": 1000}))
                .map_err(|e| Error::Other(e.to_string()))?;
        let c = to_json(&openreadout_quant::api::chromatogram(
            ds.as_mut(),
            &info,
            &q,
            &ctx,
        )?)?;
        let chrom = &c["chromatograms"][0];
        let apex = num(&chrom["apex_rt_min"], "apex_rt_min")?;
        let apex_i = num(&chrom["apex_intensity"], "apex_intensity")?;
        expect(
            close(apex, APEX_RT_MIN, 1e-9) && close(apex_i, xic_value(15), 1e-9),
            format!("XIC apex {apex_i} at {apex} min"),
        )?;
        let q: openreadout_quant::api::PeaksQuery =
            serde_json::from_value(json!({"mz": [250.0], "ppm": 10.0, "smooth": 0}))
                .map_err(|e| Error::Other(e.to_string()))?;
        let p = to_json(&openreadout_quant::api::peaks(
            ds.as_mut(),
            &info,
            &q,
            &ctx,
        )?)?;
        let peaks = p["chromatograms"][0]["peaks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        expect(
            peaks.len() == 1,
            format!("{} peaks found, 1 expected", peaks.len()),
        )?;
        let rt = num(&peaks[0]["rt_min"], "rt_min")?;
        let area = num(&peaks[0]["area"], "area")?;
        let want = expected_xic_area();
        expect(
            (rt - APEX_RT_MIN).abs() < 0.02 && close(area, want, 0.02),
            format!("peak at {rt} min, area {area:.1} (expected {want:.1})"),
        )?;
        Ok(format!(
            "XIC m/z 250 apex {APEX_RT_MIN} min; 1 peak, area {area:.1} (trapezoid {want:.1})"
        ))
    }));
    out.push(check("qpcr (rdml)", || {
        let ds = openreadout_qpcr::open_qpcr(reg, &fx.rdml)?;
        let mut req = openreadout_qpcr::QpcrReportRequest::default();
        req.compute_cq = true;
        req.threshold = Some(RDML_THRESHOLD);
        let r = to_json(&openreadout_qpcr::qpcr_report(&ds, &req)?)?;
        let recs = r["records"].as_array().cloned().unwrap_or_default();
        expect(recs.len() == 2, format!("{} records, 2 expected", recs.len()))?;
        let amp = recs
            .iter()
            .find(|x| x["sample"] == "S1")
            .ok_or_else(|| Error::Other("no record of sample S1".into()))?;
        let cq = num(&amp["cq"], "cq")?;
        let ours = num(&amp["computed_cq"], "computed_cq")?;
        expect(
            close(cq, RDML_CQ, 1e-12) && (ours - RDML_THRESHOLD_CYCLE).abs() < 0.05,
            format!(
                "stored Cq {cq}, computed Cq {ours} at threshold {RDML_THRESHOLD} (expected {RDML_THRESHOLD_CYCLE})"
            ),
        )?;
        Ok(format!(
            "2 wells; stored Cq {cq}, recomputed Cq {ours:.3} at threshold {RDML_THRESHOLD}"
        ))
    }));
    out.push(check("assay standard curve", || {
        let req: openreadout_assay::AssayRequest =
            serde_json::from_value(json!({"analysis": "curve", "model": "linear"}))
                .map_err(|e| Error::Other(e.to_string()))?;
        let r = to_json(&openreadout_assay::analyze_file(reg, &fx.plate, &req)?)?;
        let u = r["samples"]
            .as_array()
            .and_then(|s| s.iter().find(|x| x["sample"] == "U1"))
            .ok_or_else(|| Error::Other("no sample row U1".into()))?;
        let conc = num(&u["back_calculated_mean"], "back_calculated_mean")?;
        expect(
            close(conc, UNKNOWN_CONC, 1e-6),
            format!("U1 back-calculated {conc}, expected {UNKNOWN_CONC}"),
        )?;
        Ok(format!(
            "linear fit of {} standards; U1 = {conc:.4} (expected {UNKNOWN_CONC})",
            STANDARDS.len()
        ))
    }));
    out.push(check("gate (gating-ml)", || {
        let req = openreadout_fcs::analysis::GateRequest::new(
            fx.gatingml.clone(),
            Some(fcs.to_path_buf()),
        );
        let g = to_json(&openreadout_fcs::analysis::gate(&req)?)?;
        let n = g["populations"]
            .as_array()
            .and_then(|p| p.iter().find(|x| x["name"] == "Box"))
            .and_then(|x| x["count"].as_u64())
            .ok_or_else(|| Error::Other("no population Box".into()))?;
        expect(
            n == GATE_COUNT,
            format!("Box has {n} events, {GATE_COUNT} expected"),
        )?;
        Ok(format!("rectangle gate: {n} of 10 events"))
    }));
    out.push(check("jcamp-dx spectrum", || {
        let (_, d) = reg.detect(&fx.jcamp)?;
        expect(
            d.format_id == "jcamp-dx",
            format!("detected {}", d.format_id),
        )?;
        let (_, mut ds) = reg.open(&fx.jcamp)?;
        let t = ds.read_trace(0, 0, 0, JDX_POINTS as u64)?;
        let y = t
            .channels
            .first()
            .ok_or_else(|| Error::Other("no channel".into()))?;
        expect(y.len() == JDX_POINTS, format!("{} points", y.len()))?;
        for (i, v) in y.iter().enumerate() {
            expect(
                close(*v, jdx_y(i) as f64 * 0.001, 1e-9),
                format!("point {i} is {v}"),
            )?;
        }
        Ok(format!("{JDX_POINTS} absorbance points read exactly"))
    }));
    out.push(check("batch table", || {
        let files: Vec<String> = [tif, fcs, fx.mzml.as_path(), fx.jcamp.as_path()]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let a: openreadout_batch::api::BatchToolArgs =
            serde_json::from_value(json!({"measure": "info", "inputs": files}))
                .map_err(|e| Error::Other(e.to_string()))?;
        let b = to_json(&openreadout_batch::api::run_batch(reg, a, false)?)?;
        let rows = b["rows"].as_array().cloned().unwrap_or_default();
        let columns: Vec<String> = b["columns"]
            .as_array()
            .map(|c| {
                c.iter()
                    .map(|c| {
                        c.as_str()
                            .or_else(|| c["name"].as_str())
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let col = |name: &str| columns.iter().position(|c| c == name);
        let (path_col, error_col) = (col("path"), col("error"));
        let errors = rows
            .iter()
            .filter(|r| error_col.is_some_and(|i| !r[i].is_null()))
            .count();
        let inputs = rows
            .iter()
            .filter_map(|r| path_col.and_then(|i| r[i].as_str()))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        expect(
            errors == 0 && inputs == files.len(),
            format!("{} rows over {inputs} inputs, {errors} errors", rows.len()),
        )?;
        Ok(format!(
            "info over {inputs} files: {} rows, no errors",
            rows.len()
        ))
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn expected_area_is_the_gaussian_integral() {
        let analytic = APEX_HEIGHT * SIGMA_MIN * (2.0 * std::f64::consts::PI).sqrt();
        assert!(close(expected_xic_area(), analytic, 0.01));
    }
}
