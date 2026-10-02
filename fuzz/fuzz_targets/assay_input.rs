//! Plate analysis inputs (`openreadout-assay`): the text is parsed as a plate layout (grid or
//! long CSV), as a well list, and as a long plate CSV; a parsed plate is then run through every
//! analysis with that layout. Nothing may panic or hang; errors are fine.
#![no_main]

use openreadout_assay::{Analysis, AssayRequest, PlateData, Reduce, layout};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let lay = layout::parse_layout(&text, "fuzz").ok();
    if let Some(first) = text.lines().next() {
        let _ = layout::parse_wells(first);
    }
    let Ok(plate) = PlateData::from_long_csv(&text, "fuzz") else {
        return;
    };
    for analysis in [
        Analysis::Wells,
        Analysis::Curve,
        Analysis::DoseResponse,
        Analysis::Kinetics,
        Analysis::Growth,
        Analysis::Qc,
    ] {
        let req = AssayRequest {
            analysis,
            reduce: Some(Reduce::MaxSlope),
            ..AssayRequest::default()
        };
        if let Ok(out) = openreadout_assay::run(&plate, &req, lay.clone()) {
            let _ = serde_json::to_string(&out);
            let _ = openreadout_assay::render_text(&out);
        }
    }
});
