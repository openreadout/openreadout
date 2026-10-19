//! Assurance profiles (`docs/assurance.md`) of the thermal-analysis readers: the variant features
//! of a file and the feature values the development corpus validates. The tables between the
//! GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static NETZSCH_NGB: AssuranceProfile = AssuranceProfile {
    format_id: "netzsch-ngb",
    observe,
    validated: NETZSCH_NGB_VALIDATED,
    confidence: NETZSCH_NGB_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static TA_UNIVERSAL_ANALYSIS: AssuranceProfile = AssuranceProfile {
    format_id: "ta-universal-analysis",
    observe,
    validated: TA_UNIVERSAL_ANALYSIS_VALIDATED,
    confidence: TA_UNIVERSAL_ANALYSIS_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static TA_TRIOS: AssuranceProfile = AssuranceProfile {
    format_id: "ta-trios",
    observe,
    validated: TA_TRIOS_VALIDATED,
    confidence: TA_TRIOS_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for t in &info.traces {
        for c in &t.channels {
            if c.name != "time" {
                o.feature(K::Record, format!("channel {}", c.name), &[Scope::Traces]);
            }
        }
    }
    o
}

// BEGIN GENERATED netzsch-ngb (cargo xtask assurance-audit --write; do not edit)
const NETZSCH_NGB_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const NETZSCH_NGB_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 26, 5, 26),
    a::row(K::Field, "experiment.instrument.model", 23, 4, 26),
    a::row(K::FormatVersion, "db_format_1", 26, 5, 26),
    a::row(K::Instrument, "NETZSCH DIL 402 Expedis Select", 3, 1, 3),
    a::row(K::Instrument, "NETZSCH DSC 204 F1 Phoenix", 7, 1, 7),
    a::row(K::Instrument, "NETZSCH STA 449 F3 Jupiter", 7, 3, 7),
    a::row(K::Instrument, "NETZSCH STA 449F3", 4, 1, 4),
    a::row(K::Instrument, "NETZSCH TG_209F1_Libra", 3, 1, 3),
    a::row(K::Layout, "one run", 17, 3, 17),
    a::row(K::Layout, "sample and correction runs", 9, 3, 9),
    a::row(K::Record, "channel acceleration_x", 7, 3, 7),
    a::row(K::Record, "channel acceleration_y", 7, 3, 7),
    a::row(K::Record, "channel acceleration_z", 7, 3, 7),
    a::row(K::Record, "channel channel_82", 3, 1, 3),
    a::row(K::Record, "channel cooling_power", 26, 5, 26),
    a::row(K::Record, "channel dsc_signal", 20, 4, 20),
    a::row(K::Record, "channel environmental_pressure", 7, 3, 7),
    a::row(K::Record, "channel force", 3, 1, 3),
    a::row(K::Record, "channel force_setpoint", 3, 1, 3),
    a::row(K::Record, "channel furnace_power", 23, 4, 23),
    a::row(K::Record, "channel furnace_temperature", 26, 5, 26),
    a::row(K::Record, "channel h_foil_temperature", 14, 3, 14),
    a::row(K::Record, "channel length_change", 3, 1, 3),
    a::row(K::Record, "channel mass", 16, 4, 16),
    a::row(K::Record, "channel protective_flow", 25, 5, 25),
    a::row(K::Record, "channel purge_flow_1", 9, 2, 9),
    a::row(K::Record, "channel purge_flow_2", 21, 5, 21),
    a::row(K::Record, "channel sample_temperature", 26, 5, 26),
    a::row(K::Record, "channel time", 26, 5, 26),
    a::row(K::Record, "channel uc_module", 7, 3, 7),
    a::row(K::WriterVersion, "NETZSCH Proteus 6.0", 2, 1, 2),
    a::row(K::WriterVersion, "NETZSCH Proteus 8.0", 24, 5, 24),
];
// END GENERATED netzsch-ngb

// BEGIN GENERATED ta-universal-analysis (cargo xtask assurance-audit --write; do not edit)
const TA_UNIVERSAL_ANALYSIS_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const TA_UNIVERSAL_ANALYSIS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "DSC Standard - Calibration FC", 2, 1, 2),
    a::row(K::Acquisition, "DSC Standard - Modulated FC", 1, 1, 1),
    a::row(K::Acquisition, "DSC Standard Cell FC", 3, 2, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 7),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 7),
    a::row(K::FormatVersion, "Version 2.0", 6, 2, 7),
    a::row(K::Instrument, "DSC Q20", 6, 2, 7),
    a::row(K::Record, "channel cell_pressure", 2, 1, 2),
    a::row(K::Record, "channel delta_t", 2, 1, 2),
    a::row(K::Record, "channel heat_flow", 6, 2, 7),
    a::row(K::Record, "channel heat_flow_amplitude", 1, 1, 1),
    a::row(K::Record, "channel heat_flow_phase", 1, 1, 1),
    a::row(K::Record, "channel modulated_heat_flow", 1, 1, 1),
    a::row(K::Record, "channel modulated_temperature", 1, 1, 1),
    a::row(K::Record, "channel nonrev_heat_flow", 1, 1, 1),
    a::row(K::Record, "channel reference_sine_angle", 1, 1, 1),
    a::row(K::Record, "channel rev_cp", 1, 1, 1),
    a::row(K::Record, "channel rev_heat_flow", 1, 1, 1),
    a::row(K::Record, "channel sample_purge_flow", 6, 2, 7),
    a::row(K::Record, "channel temperature", 6, 2, 7),
    a::row(K::Record, "channel temperature_amplitude", 1, 1, 1),
];
// END GENERATED ta-universal-analysis

#[allow(dead_code)]
const _A: fn(K, &'static str, u32, u32, u32) -> Validated = a::row;

// BEGIN GENERATED ta-trios (cargo xtask assurance-audit --write; do not edit)
const TA_TRIOS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const TA_TRIOS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "oscillation moduli, parallel plate 12 mm", 5, 1, 5),
    a::row(K::Acquisition, "oscillation moduli, parallel plate 20 mm", 0, 0, 1),
    a::row(K::Acquisition, "oscillation moduli, parallel plate 8 mm", 4, 2, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 19),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 19),
    a::row(K::FormatVersion, "14", 15, 6, 19),
    a::row(K::FormatVersion, "generation 14", 15, 6, 19),
    a::row(K::Instrument, "DMA850", 0, 0, 1),
    a::row(K::Instrument, "DSC25", 2, 1, 2),
    a::row(K::Instrument, "DSC250", 1, 1, 1),
    a::row(K::Instrument, "DSC2500", 0, 0, 1),
    a::row(K::Instrument, "Discovery HR-2", 5, 1, 5),
    a::row(K::Instrument, "Discovery HR-3", 1, 1, 1),
    a::row(K::Instrument, "Discovery HR20", 3, 1, 3),
    a::row(K::Instrument, "Discovery HR30", 0, 0, 1),
    a::row(K::Instrument, "SDT650", 1, 1, 2),
    a::row(K::Instrument, "TGA550", 2, 1, 2),
    a::row(K::Record, "channel angular_frequency", 9, 3, 10),
    a::row(K::Record, "channel balance_purge", 2, 1, 2),
    a::row(K::Record, "channel cell_purge", 3, 2, 4),
    a::row(K::Record, "channel complex_viscosity", 9, 3, 10),
    a::row(K::Record, "channel delta_t", 3, 2, 4),
    a::row(K::Record, "channel delta_t_zero", 3, 2, 4),
    a::row(K::Record, "channel flange_temperature", 3, 2, 4),
    a::row(K::Record, "channel gap", 9, 3, 11),
    a::row(K::Record, "channel heat_capacity", 0, 0, 1),
    a::row(K::Record, "channel heat_flow", 4, 2, 5),
    a::row(K::Record, "channel heat_flow_phase", 3, 2, 4),
    a::row(K::Record, "channel heater_temperature", 3, 2, 4),
    a::row(K::Record, "channel loss_modulus", 9, 3, 10),
    a::row(K::Record, "channel oscillation_displacement", 9, 3, 10),
    a::row(K::Record, "channel oscillation_torque", 9, 3, 10),
    a::row(K::Record, "channel power_delivered", 6, 3, 7),
    a::row(K::Record, "channel ramp_rate", 2, 1, 2),
    a::row(K::Record, "channel raw_phase", 9, 3, 10),
    a::row(K::Record, "channel reference_junction_temperature", 3, 2, 4),
    a::row(K::Record, "channel sample_purge", 2, 1, 2),
    a::row(K::Record, "channel set_point_temperature", 4, 2, 4),
    a::row(K::Record, "channel signal_00df1759", 0, 0, 1),
    a::row(K::Record, "channel signal_055d9ef1", 0, 0, 1),
    a::row(K::Record, "channel signal_0657b71a", 0, 0, 1),
    a::row(K::Record, "channel signal_1a29d150", 1, 1, 1),
    a::row(K::Record, "channel signal_21d50e7b", 0, 0, 1),
    a::row(K::Record, "channel signal_2e1cdd4b", 1, 1, 1),
    a::row(K::Record, "channel signal_41018bb3", 1, 1, 1),
    a::row(K::Record, "channel signal_448bc112", 0, 0, 1),
    a::row(K::Record, "channel signal_52785362", 1, 1, 1),
    a::row(K::Record, "channel signal_532e668d", 1, 1, 1),
    a::row(K::Record, "channel signal_590f45fb", 0, 0, 1),
    a::row(K::Record, "channel signal_5a85b361", 2, 1, 2),
    a::row(K::Record, "channel signal_5f7b2d27", 9, 3, 11),
    a::row(K::Record, "channel signal_60c94927", 1, 1, 1),
    a::row(K::Record, "channel signal_649ee073", 0, 0, 1),
    a::row(K::Record, "channel signal_64ff180e", 1, 1, 1),
    a::row(K::Record, "channel signal_691f67cf", 1, 1, 1),
    a::row(K::Record, "channel signal_6f76d143", 1, 1, 1),
    a::row(K::Record, "channel signal_7b7b71f2", 1, 1, 1),
    a::row(K::Record, "channel signal_7c6223ab", 1, 1, 1),
    a::row(K::Record, "channel signal_8409266c", 1, 1, 1),
    a::row(K::Record, "channel signal_849a233d", 0, 0, 1),
    a::row(K::Record, "channel signal_95b1783d", 1, 1, 1),
    a::row(K::Record, "channel signal_98638024", 0, 0, 1),
    a::row(K::Record, "channel signal_9abe4255", 1, 1, 1),
    a::row(K::Record, "channel signal_a341c459", 0, 0, 1),
    a::row(K::Record, "channel signal_a393424e", 9, 3, 10),
    a::row(K::Record, "channel signal_a3cc3ee2", 0, 0, 1),
    a::row(K::Record, "channel signal_ac1f73ff", 0, 0, 1),
    a::row(K::Record, "channel signal_bd1abf90", 1, 1, 1),
    a::row(K::Record, "channel signal_c0b36bba", 2, 1, 2),
    a::row(K::Record, "channel signal_cf7aa154", 9, 3, 10),
    a::row(K::Record, "channel signal_d9aae222", 0, 0, 1),
    a::row(K::Record, "channel signal_dd8cab1b", 1, 1, 1),
    a::row(K::Record, "channel signal_f03e0b40", 0, 0, 1),
    a::row(K::Record, "channel step_time", 9, 3, 10),
    a::row(K::Record, "channel storage_modulus", 9, 3, 10),
    a::row(K::Record, "channel t_zero_temperature", 3, 2, 4),
    a::row(K::Record, "channel tan_delta", 9, 3, 10),
    a::row(K::Record, "channel temperature", 15, 6, 18),
    a::row(K::Record, "channel temperature_difference", 2, 1, 2),
    a::row(K::Record, "channel total_heat_capacity", 3, 2, 4),
    a::row(K::Record, "channel weight", 2, 1, 2),
    a::row(K::WriterVersion, "TRIOS 5.0", 0, 0, 1),
    a::row(K::WriterVersion, "TRIOS 5.1", 5, 1, 5),
    a::row(K::WriterVersion, "TRIOS 5.4", 3, 1, 3),
    a::row(K::WriterVersion, "TRIOS 5.5", 4, 2, 4),
    a::row(K::WriterVersion, "TRIOS 5.7", 3, 2, 5),
    a::row(K::WriterVersion, "TRIOS 5.9", 0, 0, 1),
];
// END GENERATED ta-trios
