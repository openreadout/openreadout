#![allow(clippy::float_cmp)] // values built from exact binary fractions
//! Gating, compensation and transforms on a synthetic FCS file whose populations can be counted
//! by hand, with a FlowJo-style workspace and a Gating-ML document written for it.

use std::path::{Path, PathBuf};

use openreadout_core::Error;
use openreadout_fcs::analysis::{
    CompensationChoice, GateRequest, TableOptions, TransformChoice, gate, parse_transform_spec,
    processed_rows,
};

/// FCS 3.1, float, little-endian, parameters FSC-A, FL1-A, FL2-A (FSC-A with `$P1G/2/`), and a
/// `$SPILLOVER` of 10 % FL1 → FL2.
fn fcs(dir: &Path, events: &[[f32; 3]]) -> PathBuf {
    let kws: Vec<(&str, String)> = vec![
        ("$BYTEORD", "1,2,3,4".into()),
        ("$DATATYPE", "F".into()),
        ("$MODE", "L".into()),
        ("$PAR", "3".into()),
        ("$TOT", events.len().to_string()),
        ("$NEXTDATA", "0".into()),
        ("$BEGINANALYSIS", "0".into()),
        ("$ENDANALYSIS", "0".into()),
        ("$BEGINSTEXT", "0".into()),
        ("$ENDSTEXT", "0".into()),
        ("$FIL", "synthetic.fcs".into()),
        ("$P1N", "FSC-A".into()),
        ("$P1B", "32".into()),
        ("$P1E", "0,0".into()),
        ("$P1R", "262144".into()),
        ("$P1G", "2".into()),
        ("$P2N", "FL1-A".into()),
        ("$P2S", "CD3 FITC".into()),
        ("$P2B", "32".into()),
        ("$P2E", "0,0".into()),
        ("$P2R", "262144".into()),
        ("$P3N", "FL2-A".into()),
        ("$P3B", "32".into()),
        ("$P3E", "0,0".into()),
        ("$P3R", "262144".into()),
        ("$SPILLOVER", "2,FL1-A,FL2-A,1,0.1,0,1".into()),
    ];
    let mut data = Vec::new();
    for e in events {
        for v in e {
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let render = |begin: u64, end: u64| {
        let mut t = String::from("/");
        for (k, v) in kws
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .chain([
                ("$BEGINDATA".to_string(), format!("{begin:010}")),
                ("$ENDDATA".to_string(), format!("{end:010}")),
            ])
        {
            t.push_str(&k);
            t.push('/');
            t.push_str(&v);
            t.push('/');
        }
        t
    };
    let text_len = render(0, 0).len() as u64;
    let (tb, te) = (58u64, 58 + text_len - 1);
    let (db, de) = (te + 1, te + data.len() as u64);
    let mut out = format!("FCS3.1    {tb:>8}{te:>8}{db:>8}{de:>8}{:>8}{:>8}", 0, 0).into_bytes();
    out.extend_from_slice(render(db, de).as_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(b"00000000");
    let p = dir.join("synthetic.fcs");
    std::fs::write(&p, out).unwrap();
    p
}

/// Raw events: FSC-A is stored ×2 (gain 2); FL2-A carries 10 % of FL1-A.
fn events() -> Vec<[f32; 3]> {
    // (scale FSC, true FL1, true FL2)
    let truth = [
        (100.0, 1000.0, 50.0),
        (200.0, 1000.0, 500.0),
        (300.0, 10.0, 5000.0),
        (400.0, 20000.0, 20.0),
        (500.0, 5.0, 5.0),
        (600.0, 3000.0, 3000.0),
    ];
    truth
        .iter()
        .map(|&(f, a, b)| [f * 2.0, a, b + 0.1 * a])
        .collect()
}

const WSP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Workspace version="20.0" flowJoVersion="10.8.1" xmlns:gating="http://www.isac-net.org/std/Gating-ML/v2.0/gating" xmlns:transforms="http://www.isac-net.org/std/Gating-ML/v2.0/transformations" xmlns:data-type="http://www.isac-net.org/std/Gating-ML/v2.0/datatypes">
 <Groups><GroupNode name="All Samples"><Group><SampleRefs><SampleRef sampleID="7"/></SampleRefs></Group></GroupNode></Groups>
 <SampleList>
  <Sample>
   <DataSet uri="file:/data/synthetic.fcs" sampleID="7"/>
   <transforms:spilloverMatrix spectral="0" prefix="Comp-" name="Acquisition-defined" suffix="">
    <data-type:parameters><data-type:parameter data-type:name="FL1-A"/><data-type:parameter data-type:name="FL2-A"/></data-type:parameters>
    <transforms:spillover data-type:parameter="FL1-A"><transforms:coefficient data-type:parameter="FL1-A" transforms:value="1"/><transforms:coefficient data-type:parameter="FL2-A" transforms:value="0.1"/></transforms:spillover>
    <transforms:spillover data-type:parameter="FL2-A"><transforms:coefficient data-type:parameter="FL1-A" transforms:value="0"/><transforms:coefficient data-type:parameter="FL2-A" transforms:value="1"/></transforms:spillover>
   </transforms:spilloverMatrix>
   <Transformations>
    <transforms:linear transforms:minRange="0" transforms:maxRange="262144"><data-type:parameter data-type:name="FSC-A"/></transforms:linear>
    <transforms:logicle transforms:length="256" transforms:T="262144" transforms:A="0" transforms:W="0.5" transforms:M="4.5"><data-type:parameter data-type:name="FL1-A"/></transforms:logicle>
    <transforms:logicle transforms:length="256" transforms:T="262144" transforms:A="0" transforms:W="0.5" transforms:M="4.5"><data-type:parameter data-type:name="FL2-A"/></transforms:logicle>
   </Transformations>
   <Keywords><Keyword name="$FIL" value="synthetic.fcs"/></Keywords>
   <SampleNode name="synthetic.fcs" sampleID="7" count="6">
    <Subpopulations>
     <Population name="Cells" owningGroup="" count="5">
      <Gate><gating:RectangleGate eventsInside="1"><gating:dimension gating:min="150"><data-type:fcs-dimension data-type:name="FSC-A"/></gating:dimension></gating:RectangleGate></Gate>
      <Subpopulations>
       <Population name="FL1+" owningGroup="" count="3">
        <Gate><gating:RectangleGate eventsInside="1"><gating:dimension gating:min="500"><data-type:fcs-dimension data-type:name="Comp-FL1-A"/></gating:dimension></gating:RectangleGate></Gate>
       </Population>
       <Population name="FL2 high" owningGroup="" count="2">
        <Gate><gating:PolygonGate eventsInside="1">
         <gating:dimension><data-type:fcs-dimension data-type:name="Comp-FL1-A"/></gating:dimension>
         <gating:dimension><data-type:fcs-dimension data-type:name="Comp-FL2-A"/></gating:dimension>
         <gating:vertex><gating:coordinate data-type:value="0"/><gating:coordinate data-type:value="1000"/></gating:vertex>
         <gating:vertex><gating:coordinate data-type:value="100000"/><gating:coordinate data-type:value="1000"/></gating:vertex>
         <gating:vertex><gating:coordinate data-type:value="100000"/><gating:coordinate data-type:value="100000"/></gating:vertex>
         <gating:vertex><gating:coordinate data-type:value="0"/><gating:coordinate data-type:value="100000"/></gating:vertex>
        </gating:PolygonGate></Gate>
       </Population>
       <Population name="Not small" owningGroup="" count="4">
        <Gate><gating:RectangleGate eventsInside="0"><gating:dimension gating:max="250"><data-type:fcs-dimension data-type:name="FSC-A"/></gating:dimension></gating:RectangleGate></Gate>
       </Population>
       <NotNode name="FL1-" owningGroup="" count="2">
        <Dependents><Dependent name="Cells/FL1+"/></Dependents>
       </NotNode>
      </Subpopulations>
     </Population>
    </Subpopulations>
   </SampleNode>
  </Sample>
 </SampleList>
</Workspace>
"#;

const GML: &str = r#"<?xml version="1.0"?>
<gating:Gating-ML xmlns:gating="http://www.isac-net.org/std/Gating-ML/v2.0/gating" xmlns:transforms="http://www.isac-net.org/std/Gating-ML/v2.0/transformations" xmlns:data-type="http://www.isac-net.org/std/Gating-ML/v2.0/datatypes">
 <transforms:transformation transforms:id="L"><transforms:logicle transforms:T="262144" transforms:W="0.5" transforms:M="4.5" transforms:A="0"/></transforms:transformation>
 <gating:RectangleGate gating:id="Cells"><gating:dimension gating:compensation-ref="uncompensated" gating:min="150"><data-type:fcs-dimension data-type:name="FSC-A"/></gating:dimension></gating:RectangleGate>
 <gating:QuadrantGate gating:id="Q" gating:parent_id="Cells">
  <gating:divider gating:id="x" gating:compensation-ref="FCS" gating:transformation-ref="L"><data-type:fcs-dimension data-type:name="FL1-A"/><gating:value>0.5</gating:value></gating:divider>
  <gating:divider gating:id="y" gating:compensation-ref="FCS" gating:transformation-ref="L"><data-type:fcs-dimension data-type:name="FL2-A"/><gating:value>0.5</gating:value></gating:divider>
  <gating:Quadrant gating:id="PP"><gating:position gating:divider_ref="x" gating:location="0.9"/><gating:position gating:divider_ref="y" gating:location="0.9"/></gating:Quadrant>
  <gating:Quadrant gating:id="PN"><gating:position gating:divider_ref="x" gating:location="0.9"/><gating:position gating:divider_ref="y" gating:location="0.1"/></gating:Quadrant>
 </gating:QuadrantGate>
 <gating:BooleanGate gating:id="NotPP"><gating:and><gating:gateReference gating:ref="Cells"/><gating:gateReference gating:ref="PP" gating:use-as-complement="true"/></gating:and></gating:BooleanGate>
</gating:Gating-ML>
"#;

fn counts(out: &openreadout_core::flow::GateOutput) -> Vec<(String, u64)> {
    out.populations
        .iter()
        .map(|r| (r.path.clone(), r.count.unwrap()))
        .collect()
}

#[test]
fn workspace_populations_count_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let f = fcs(dir.path(), &events());
    let w = dir.path().join("a.wsp");
    std::fs::write(&w, WSP).unwrap();
    let out = gate(&GateRequest::new(&w, Some(f))).unwrap();
    assert_eq!(out.sample.as_ref().unwrap().id, "7");
    let c = counts(&out);
    // Cells: scale FSC >= 150 → events 2..6 (FSC stored ×2, $P1G/2/ divides back).
    assert!(c.contains(&("/Cells".into(), 5)), "{c:?}");
    // Compensated FL1 >= 500 among cells: 1000, 20000, 3000.
    assert!(c.contains(&("/Cells/FL1+".into(), 3)), "{c:?}");
    // Compensated FL2 >= 1000 among cells: 5000 and 3000 (the 500 + 100 of event 2 is not).
    assert!(c.contains(&("/Cells/FL2 high".into(), 2)), "{c:?}");
    // Outside FSC < 250 among cells: 300, 400, 500, 600.
    assert!(c.contains(&("/Cells/Not small".into(), 4)), "{c:?}");
    assert!(c.contains(&("/Cells/FL1-".into(), 2)), "{c:?}");
    // The stored counts agree, so no note about differences.
    assert!(
        !out.notes.iter().any(|n| n.contains("differ")),
        "{:?}",
        out.notes
    );
    assert_eq!(out.tree.len(), 1);
    assert_eq!(out.tree[0].children.len(), 4);
    // --population limits the report to a subtree.
    let mut req = GateRequest::new(&w, None);
    req.populations = vec!["FL1+".into()];
    req.sample = Some("synthetic.fcs".into());
    let only = gate(&req).unwrap();
    assert_eq!(only.populations.len(), 1);
    assert!(only.populations[0].count.is_none());
}

#[test]
fn gating_ml_quadrants_booleans_and_file_compensation() {
    let dir = tempfile::tempdir().unwrap();
    let f = fcs(dir.path(), &events());
    let g = dir.path().join("g.xml");
    std::fs::write(&g, GML).unwrap();
    let out = gate(&GateRequest::new(&g, Some(f))).unwrap();
    let c = counts(&out);
    // logicle(0.5) threshold ≈ 1.9e3 (T=262144, W=0.5, M=4.5): among cells, FL1 ≥ it: 20000,
    // 3000; FL2 ≥ it: 5000, 3000.
    assert!(c.contains(&("/Cells/Q/PP".into(), 1)), "{c:?}");
    assert!(c.contains(&("/Cells/Q/PN".into(), 1)), "{c:?}");
    assert!(c.contains(&("/NotPP".into(), 4)), "{c:?}");
    assert!(out.compensation.iter().any(|m| m.source == "fcs" && m.used));
    assert!(
        out.tree
            .iter()
            .any(|n| n.children.iter().any(|q| q.gate_type == "quadrant-gate"))
    );
}

#[test]
fn table_compensation_transform_and_membership() {
    let dir = tempfile::tempdir().unwrap();
    let f = fcs(dir.path(), &events());
    let w = dir.path().join("a.wsp");
    std::fs::write(&w, WSP).unwrap();
    let mut opts = TableOptions::default();
    opts.compensation = Some(CompensationChoice::File);
    let r = processed_rows(&f, 0, 0, 100, &opts).unwrap();
    assert_eq!(r.columns[0][1], 200.0); // FSC ÷ $P1G
    assert!((r.columns[2][1] - 500.0).abs() < 1e-9); // FL2 compensated
    assert_eq!(r.processing.scale.len(), 1);
    let mut opts = TableOptions::default();
    opts.compensation = Some(CompensationChoice::Auto);
    opts.gating_file = Some(w.clone());
    opts.transform = Some(TransformChoice::Uniform {
        transform: parse_transform_spec("arcsinh-cofactor:150").unwrap(),
        parameters: vec!["FL1-A".into()],
    });
    opts.populations = vec!["/Cells/FL1+".into()];
    let r = processed_rows(&f, 0, 0, 100, &opts).unwrap();
    assert_eq!(
        r.processing.compensation.as_ref().unwrap().source,
        "workspace"
    );
    assert!((r.columns[1][0] - (1000.0f64 / 150.0).asinh()).abs() < 1e-12);
    assert_eq!(r.names.last().unwrap(), "gate:/Cells/FL1+");
    assert_eq!(r.columns[3], vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    assert_eq!(r.processing.gates[0].count_in_rows, 3);
    let mut opts = TableOptions::default();
    opts.transform = Some(TransformChoice::GatingFile);
    opts.gating_file = Some(w);
    let r = processed_rows(&f, 0, 0, 100, &opts).unwrap();
    assert_eq!(r.processing.transforms.len(), 3);
}

#[test]
fn errors_are_clean() {
    let dir = tempfile::tempdir().unwrap();
    let f = fcs(dir.path(), &events());
    // Malformed XML: corrupt, exit 4.
    let bad = dir.path().join("bad.wsp");
    std::fs::write(&bad, "<Workspace><SampleList><Sample>").unwrap();
    let e = gate(&GateRequest::new(&bad, Some(f.clone()))).unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    // Not a gating file at all.
    let other = dir.path().join("x.xml");
    std::fs::write(&other, "<?xml version=\"1.0\"?><OME/>").unwrap();
    assert_eq!(
        gate(&GateRequest::new(&other, Some(f.clone())))
            .unwrap_err()
            .exit_code(),
        6
    );
    // A gate on a parameter the file does not have: usage error naming it.
    let missing = WSP.replace(
        "Comp-FL1-A\"/></gating:dimension></gating:RectangleGate>",
        "Comp-FL9-A\"/></gating:dimension></gating:RectangleGate>",
    );
    let w = dir.path().join("m.wsp");
    std::fs::write(&w, missing).unwrap();
    let e = gate(&GateRequest::new(&w, Some(f.clone()))).unwrap_err();
    assert!(
        matches!(e, Error::Usage(ref m) if m.contains("FL9-A")),
        "{e}"
    );
    // An unknown FlowJo transform on a gated parameter: unsupported, exit 6.
    let odd = WSP.replace(
        "<transforms:linear transforms:minRange=\"0\" transforms:maxRange=\"262144\"><data-type:parameter data-type:name=\"FSC-A\"/></transforms:linear>",
        "<transforms:miracle><data-type:parameter data-type:name=\"FSC-A\"/></transforms:miracle>",
    );
    let w = dir.path().join("o.wsp");
    std::fs::write(&w, odd).unwrap();
    assert_eq!(
        gate(&GateRequest::new(&w, Some(f.clone())))
            .unwrap_err()
            .exit_code(),
        6
    );
    // Membership columns need a gating file.
    let mut opts = TableOptions::default();
    opts.populations = vec!["/Cells".into()];
    assert_eq!(
        processed_rows(&f, 0, 0, 10, &opts).unwrap_err().exit_code(),
        2
    );
    // A bad transform spec is a usage error.
    assert_eq!(
        parse_transform_spec("logicle:T=abc")
            .unwrap_err()
            .exit_code(),
        2
    );
}
