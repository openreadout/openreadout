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
const NETZSCH_NGB_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const NETZSCH_NGB_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 18, 2, 18),
    a::row(K::Field, "experiment.instrument.model", 18, 2, 18),
    a::row(K::FormatVersion, "db_format_1", 18, 2, 18),
    a::row(K::Instrument, "NETZSCH DIL 402 Expedis Select", 3, 1, 3),
    a::row(K::Instrument, "NETZSCH DSC 204 F1 Phoenix", 7, 1, 7),
    a::row(K::Instrument, "NETZSCH STA 449 F3 Jupiter", 2, 1, 2),
    a::row(K::Instrument, "NETZSCH STA 449F3", 4, 1, 4),
    a::row(K::Layout, "one run", 14, 2, 14),
    a::row(K::Layout, "sample and correction runs", 4, 1, 4),
    a::row(K::Record, "channel acceleration_x", 2, 1, 2),
    a::row(K::Record, "channel acceleration_y", 2, 1, 2),
    a::row(K::Record, "channel acceleration_z", 2, 1, 2),
    a::row(K::Record, "channel channel_82", 3, 1, 3),
    a::row(K::Record, "channel cooling_power", 18, 2, 18),
    a::row(K::Record, "channel dsc_signal", 15, 2, 15),
    a::row(K::Record, "channel environmental_pressure", 2, 1, 2),
    a::row(K::Record, "channel force", 3, 1, 3),
    a::row(K::Record, "channel force_setpoint", 3, 1, 3),
    a::row(K::Record, "channel furnace_power", 18, 2, 18),
    a::row(K::Record, "channel furnace_temperature", 18, 2, 18),
    a::row(K::Record, "channel h_foil_temperature", 9, 1, 9),
    a::row(K::Record, "channel length_change", 3, 1, 3),
    a::row(K::Record, "channel mass", 8, 1, 8),
    a::row(K::Record, "channel protective_flow", 18, 2, 18),
    a::row(K::Record, "channel purge_flow_1", 6, 1, 6),
    a::row(K::Record, "channel purge_flow_2", 13, 2, 13),
    a::row(K::Record, "channel sample_temperature", 18, 2, 18),
    a::row(K::Record, "channel time", 18, 2, 18),
    a::row(K::Record, "channel uc_module", 2, 1, 2),
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
const TA_TRIOS_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const TA_TRIOS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "oscillation moduli, parallel plate 12 mm", 5, 1, 5),
    a::row(K::Acquisition, "oscillation moduli, parallel plate 20 mm", 0, 0, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 12),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 12),
    a::row(K::FormatVersion, "14", 9, 3, 12),
    a::row(K::FormatVersion, "generation 14", 9, 3, 12),
    a::row(K::Instrument, "DMA850", 0, 0, 1),
    a::row(K::Instrument, "DSC25", 2, 1, 2),
    a::row(K::Instrument, "DSC2500", 0, 0, 1),
    a::row(K::Instrument, "Discovery HR-2", 5, 1, 5),
    a::row(K::Instrument, "Discovery HR30", 0, 0, 1),
    a::row(K::Instrument, "TGA550", 2, 1, 2),
    a::row(K::Record, "channel angular_frequency", 5, 1, 6),
    a::row(K::Record, "channel balance_purge", 2, 1, 2),
    a::row(K::Record, "channel cell_purge", 2, 1, 3),
    a::row(K::Record, "channel complex_viscosity", 5, 1, 6),
    a::row(K::Record, "channel delta_t", 2, 1, 3),
    a::row(K::Record, "channel delta_t_zero", 2, 1, 3),
    a::row(K::Record, "channel flange_temperature", 2, 1, 3),
    a::row(K::Record, "channel gap", 5, 1, 7),
    a::row(K::Record, "channel heat_capacity", 0, 0, 1),
    a::row(K::Record, "channel heat_flow", 2, 1, 3),
    a::row(K::Record, "channel heat_flow_phase", 2, 1, 3),
    a::row(K::Record, "channel heater_temperature", 2, 1, 3),
    a::row(K::Record, "channel loss_modulus", 5, 1, 6),
    a::row(K::Record, "channel oscillation_displacement", 5, 1, 6),
    a::row(K::Record, "channel oscillation_torque", 5, 1, 6),
    a::row(K::Record, "channel power_delivered", 4, 2, 5),
    a::row(K::Record, "channel ramp_rate", 2, 1, 2),
    a::row(K::Record, "channel raw_phase", 5, 1, 6),
    a::row(K::Record, "channel reference_junction_temperature", 2, 1, 3),
    a::row(K::Record, "channel sample_purge", 2, 1, 2),
    a::row(K::Record, "channel set_point_temperature", 2, 1, 2),
    a::row(K::Record, "channel signal_00df1759", 0, 0, 1),
    a::row(K::Record, "channel signal_055d9ef1", 0, 0, 1),
    a::row(K::Record, "channel signal_0657b71a", 0, 0, 1),
    a::row(K::Record, "channel signal_21d50e7b", 0, 0, 1),
    a::row(K::Record, "channel signal_448bc112", 0, 0, 1),
    a::row(K::Record, "channel signal_590f45fb", 0, 0, 1),
    a::row(K::Record, "channel signal_5f7b2d27", 5, 1, 7),
    a::row(K::Record, "channel signal_649ee073", 0, 0, 1),
    a::row(K::Record, "channel signal_849a233d", 0, 0, 1),
    a::row(K::Record, "channel signal_98638024", 0, 0, 1),
    a::row(K::Record, "channel signal_a341c459", 0, 0, 1),
    a::row(K::Record, "channel signal_a393424e", 5, 1, 6),
    a::row(K::Record, "channel signal_a3cc3ee2", 0, 0, 1),
    a::row(K::Record, "channel signal_ac1f73ff", 0, 0, 1),
    a::row(K::Record, "channel signal_c0b36bba", 2, 1, 2),
    a::row(K::Record, "channel signal_cf7aa154", 5, 1, 6),
    a::row(K::Record, "channel signal_d9aae222", 0, 0, 1),
    a::row(K::Record, "channel signal_f03e0b40", 0, 0, 1),
    a::row(K::Record, "channel step_time", 5, 1, 6),
    a::row(K::Record, "channel storage_modulus", 5, 1, 6),
    a::row(K::Record, "channel t_zero_temperature", 2, 1, 3),
    a::row(K::Record, "channel tan_delta", 5, 1, 6),
    a::row(K::Record, "channel temperature", 9, 3, 12),
    a::row(K::Record, "channel temperature_difference", 2, 1, 2),
    a::row(K::Record, "channel total_heat_capacity", 2, 1, 3),
    a::row(K::Record, "channel weight", 2, 1, 2),
    a::row(K::WriterVersion, "TRIOS 5.1", 5, 1, 5),
    a::row(K::WriterVersion, "TRIOS 5.5", 2, 1, 2),
    a::row(K::WriterVersion, "TRIOS 5.7", 2, 1, 4),
    a::row(K::WriterVersion, "TRIOS 5.9", 0, 0, 1),
];
// END GENERATED ta-trios
