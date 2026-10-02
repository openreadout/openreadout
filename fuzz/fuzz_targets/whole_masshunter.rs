//! Whole data set, Agilent MassHunter `.d` directory: `AcqData/` scan index schema, scan index,
//! centroid and profile data, calibrations, time segments, contents, devices, sample info and one
//! LC signal pair; parts separated by `openreadout_fuzz::BUNDLE_SEP`, missing parts written empty.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_agilent_ms::AgilentMsReader,
        "whole_masshunter",
        "s.d",
        &[
            "s.d/AcqData/MSScan.xsd",
            "s.d/AcqData/MSScan.bin",
            "s.d/AcqData/MSPeak.bin",
            "s.d/AcqData/MSProfile.bin",
            "s.d/AcqData/MSMassCal.bin",
            "s.d/AcqData/DefaultMassCal.xml",
            "s.d/AcqData/MSTS.xml",
            "s.d/AcqData/Contents.xml",
            "s.d/AcqData/Devices.xml",
            "s.d/AcqData/sample_info.xml",
            "s.d/AcqData/TCC1.cd",
            "s.d/AcqData/TCC1.cg",
        ],
        None,
        data,
    );
});
