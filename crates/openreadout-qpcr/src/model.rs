//! The normalized qPCR model every dialect is parsed into (names: `docs/formats/qpcr.md`).
//!
//! A file holds shared definitions (samples, targets, dyes, thermal programs) and one or more
//! plate runs; a run holds reactions (wells); a reaction holds one assay per target measured in
//! it (the vendor's result and the curves of that target's reporter dye) and the per-dye
//! multicomponent signals where the file keeps them.

use serde_json::Value;

/// Which dialect a file was read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// RDML (any version 1.0-1.4).
    Rdml,
    /// Applied Biosystems `.eds`, `apldbio/sds/` XML layout (QuantStudio 3/5/6/7/12K, ViiA 7).
    EdsSds,
    /// Applied Biosystems `.eds`, 7500 / StepOne layout (`multicomponent_data.txt`).
    Eds7500,
    /// Applied Biosystems `.eds`, JSON layout (Design & Analysis 2, QuantStudio 1/3/5/6/7 Pro).
    EdsJson,
    /// Qiagen Rotor-Gene `.rex`.
    Rex,
    /// Roche LightCycler 480 `.ixo`.
    Ixo,
}

impl Dialect {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Dialect::Rdml => "rdml",
            Dialect::EdsSds => "eds-sds",
            Dialect::Eds7500 => "eds-7500",
            Dialect::EdsJson => "eds-json",
            Dialect::Rex => "rex",
            Dialect::Ixo => "ixo",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Instrument {
    pub(crate) manufacturer: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) serial_number: Option<String>,
    pub(crate) software: Option<String>,
    pub(crate) software_version: Option<String>,
    pub(crate) firmware_version: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Person {
    pub(crate) id: String,
    pub(crate) first_name: Option<String>,
    pub(crate) last_name: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) lab: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Sample {
    pub(crate) name: String,
    /// The file's identifier when the name shown is another (an RDML sample whose `id` is a
    /// GUID and whose description names it: LightCycler 96).
    pub(crate) id: Option<String>,
    /// Our vocabulary: unknown, standard, ntc, nac, ntp, nrt, positive, calibrator, ...
    pub(crate) kind: Option<String>,
    pub(crate) quantity: Option<f64>,
    pub(crate) quantity_unit: Option<String>,
    pub(crate) description: Option<String>,
    /// Free `property = value` annotations (RDML `annotation`, `.eds` custom properties).
    pub(crate) annotations: Vec<(String, String)>,
    /// Target-specific kinds and quantities (RDML 1.3 `type`/`quantity` with `targetId`).
    pub(crate) per_target: Vec<(String, Option<String>, Option<f64>)>,
}

impl Sample {
    /// Kind of this sample for `target` (a target-specific entry, else the general one).
    pub(crate) fn kind_for(&self, target: Option<&str>) -> Option<String> {
        target
            .and_then(|t| self.per_target.iter().find(|p| p.0 == t))
            .and_then(|p| p.1.clone())
            .or_else(|| self.kind.clone())
    }

    /// Quantity of this sample for `target`.
    pub(crate) fn quantity_for(&self, target: Option<&str>) -> Option<f64> {
        target
            .and_then(|t| self.per_target.iter().find(|p| p.0 == t))
            .and_then(|p| p.2)
            .or(self.quantity)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Target {
    pub(crate) name: String,
    /// `reference` or `of interest` (RDML `ref`/`toi`).
    pub(crate) kind: Option<String>,
    pub(crate) dye: Option<String>,
    pub(crate) quencher: Option<String>,
    /// Amplification efficiency as a fold per cycle (2.0 = 100 %).
    pub(crate) efficiency: Option<f64>,
    pub(crate) efficiency_se: Option<f64>,
    pub(crate) efficiency_method: Option<String>,
    pub(crate) melting_temperature: Option<f64>,
    pub(crate) description: Option<String>,
    pub(crate) sequences: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Dye {
    pub(crate) name: String,
    pub(crate) chemistry: Option<String>,
}

/// One step of a thermal program.
#[derive(Debug, Clone, Default)]
pub(crate) struct Step {
    /// `temperature`, `gradient` (a melt or ramp between two temperatures), `loop`, `pause`,
    /// `lid open`.
    pub(crate) kind: String,
    pub(crate) temperature_c: Option<f64>,
    pub(crate) high_temperature_c: Option<f64>,
    pub(crate) low_temperature_c: Option<f64>,
    pub(crate) hold_s: Option<f64>,
    pub(crate) ramp_c_per_s: Option<f64>,
    /// `real time` (read at the end of the step), `melt` (read during the ramp).
    pub(crate) measure: Option<String>,
    /// Loops: go to step number `goto` (1-based, program-wide) `repeat` more times.
    pub(crate) goto: Option<u32>,
    pub(crate) repeat: Option<u32>,
}

/// A stage: steps repeated `repeats` times (RDML programs are one flat stage with loops).
#[derive(Debug, Clone, Default)]
pub(crate) struct Stage {
    /// `hold`, `cycling`, `melt`, `pre-read`, ... (our vocabulary), or the RDML flat program.
    pub(crate) kind: String,
    pub(crate) repeats: u32,
    pub(crate) steps: Vec<Step>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Program {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) lid_temperature_c: Option<f64>,
    pub(crate) sample_volume_ul: Option<f64>,
    pub(crate) run_mode: Option<String>,
    pub(crate) stages: Vec<Stage>,
}

impl Program {
    /// Temperature of the step that reads fluorescence in the cycling stage (the annealing /
    /// extension step of a two- or three-step PCR); `None` when no such step is known.
    pub(crate) fn acquisition_temperature(&self) -> Option<f64> {
        let mut stages = self
            .stages
            .iter()
            .filter(|s| s.repeats > 1 || s.kind == "cycling");
        stages
            .find_map(|s| {
                s.steps
                    .iter()
                    .find(|st| st.measure.as_deref() == Some("real time"))
                    .and_then(|st| st.temperature_c)
            })
            .or_else(|| {
                self.stages.iter().find_map(|s| {
                    s.steps
                        .iter()
                        .find(|st| st.measure.as_deref() == Some("real time"))
                        .and_then(|st| st.temperature_c)
                })
            })
    }

    /// Temperatures of every step of the cycling stage that reads fluorescence (a three-step
    /// protocol may read at both the annealing and the extension step).
    pub(crate) fn acquisition_temperatures(&self) -> Vec<f64> {
        self.stages
            .iter()
            .find(|s| s.kind == "cycling" || s.repeats > 1)
            .map(|s| {
                s.steps
                    .iter()
                    .filter(|st| st.measure.as_deref() == Some("real time"))
                    .filter_map(|st| st.temperature_c)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Number of amplification cycles: the repeat count of the cycling stage, or the loop that
    /// jumps back over the real-time read (RDML: `repeat` more times, so cycles = repeat + 1).
    pub(crate) fn cycles(&self) -> Option<u32> {
        for s in &self.stages {
            if s.kind == "cycling" && s.repeats > 0 {
                return Some(s.repeats);
            }
            for st in &s.steps {
                if st.kind == "loop"
                    && let Some(r) = st.repeat
                {
                    return r.checked_add(1);
                }
            }
        }
        None
    }
}

/// An amplification curve: one value per cycle.
#[derive(Debug, Clone, Default)]
pub(crate) struct Curve {
    pub(crate) cycles: Vec<f64>,
    /// The fluorescence as the file keeps it (`Rn`, the multicomponent reporter signal, or
    /// RDML `adp/fluor`).
    pub(crate) fluorescence: Vec<f64>,
    /// Baseline-corrected values computed by the vendor software (`ΔRn`), when stored.
    pub(crate) corrected: Option<Vec<f64>>,
    /// What `fluorescence` is: `Rn`, `multicomponent`, `fluorescence`.
    pub(crate) quantity: &'static str,
}

/// A melt curve.
#[derive(Debug, Clone, Default)]
pub(crate) struct Melt {
    pub(crate) temperature: Vec<f64>,
    pub(crate) fluorescence: Vec<f64>,
    /// −dF/dT from the vendor software, on its own temperature grid.
    pub(crate) derivative: Option<(Vec<f64>, Vec<f64>)>,
}

/// One target measured in one reaction, with the vendor's result.
#[derive(Debug, Clone, Default)]
pub(crate) struct Assay {
    pub(crate) target: Option<String>,
    pub(crate) dye: Option<String>,
    /// Role of the well for this target (our vocabulary, as `Sample::kind`).
    pub(crate) task: Option<String>,
    pub(crate) quantity: Option<f64>,
    pub(crate) cq: Option<f64>,
    /// Set when the vendor reported "no Cq" (undetermined, `-1`, `NaN`, or in SDS/7500 text
    /// results a `Ct` equal to the cycle count) rather than no result.
    pub(crate) cq_undetermined: bool,
    /// The number the file stores for an undetermined result (SDS/7500: the cycle count).
    pub(crate) cq_stored: Option<f64>,
    pub(crate) cq_mean: Option<f64>,
    pub(crate) cq_sd: Option<f64>,
    pub(crate) threshold: Option<f64>,
    /// `threshold` is the one the Cq was called at (a per-well result, RDML `quantFluor`, or a
    /// manual setting), not merely a setting that an automatic threshold overrode.
    pub(crate) threshold_used: bool,
    pub(crate) auto_threshold: Option<bool>,
    pub(crate) baseline_start: Option<u32>,
    pub(crate) baseline_end: Option<u32>,
    pub(crate) auto_baseline: Option<bool>,
    pub(crate) amp_status: Option<String>,
    pub(crate) cq_confidence: Option<f64>,
    pub(crate) tm: Vec<f64>,
    pub(crate) efficiency: Option<f64>,
    pub(crate) efficiency_se: Option<f64>,
    pub(crate) n0: Option<f64>,
    /// Quantity the vendor software calculated (standard curve).
    pub(crate) calculated_quantity: Option<f64>,
    pub(crate) excluded: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) flags: Vec<String>,
    pub(crate) background: Option<f64>,
    /// Slope of the background line (RDML `bgFluorSlp`).
    pub(crate) background_slope: Option<f64>,
    /// Threshold recovered from the vendor's own ΔRn at its Cq (the SDS layout does not store
    /// automatic thresholds); used as the default threshold of our Cq.
    pub(crate) estimated_threshold: Option<f64>,
    /// ΔCq, RQ and ΔΔCq as the vendor software computed them (ΔΔCt experiments).
    pub(crate) vendor_delta_cq: Option<f64>,
    pub(crate) vendor_rq: Option<f64>,
    pub(crate) vendor_delta_delta_cq: Option<f64>,
    pub(crate) amplification: Option<Curve>,
    pub(crate) melt: Option<Melt>,
}

/// A dye's signal at every amplification read, as the file stores it.
#[derive(Debug, Clone, Default)]
pub(crate) struct DyeSignal {
    pub(crate) dye: String,
    pub(crate) values: Vec<f64>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Reaction {
    /// Zero-based position, row-major.
    pub(crate) position: u32,
    pub(crate) row: u32,
    pub(crate) column: u32,
    pub(crate) sample: Option<String>,
    pub(crate) omitted: bool,
    pub(crate) assays: Vec<Assay>,
    pub(crate) signals: Vec<DyeSignal>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Run {
    pub(crate) name: String,
    pub(crate) experiment: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) instrument: Option<String>,
    pub(crate) software: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) program: Option<usize>,
    pub(crate) rows: u32,
    pub(crate) columns: u32,
    /// RDML label formats: `ABC`, `123`, `A1a1`.
    pub(crate) row_label: String,
    pub(crate) column_label: String,
    pub(crate) cq_method: Option<String>,
    pub(crate) background_method: Option<String>,
    pub(crate) reactions: Vec<Reaction>,
}

impl Run {
    /// Well name of a zero-based position (`A1`, `P24`, `AF48`; `12` for numeric labels).
    pub(crate) fn well_name(&self, row: u32, column: u32) -> String {
        // Rotors and strips (numeric labels, one row or one column): the tube number.
        if self.row_label == "123" && self.column_label == "123" {
            if self.columns == 1 {
                return (row + 1).to_string();
            }
            if self.rows == 1 {
                return (column + 1).to_string();
            }
        }
        well_name(row, column, &self.row_label, &self.column_label)
    }
}

/// `A`..`Z`, then `AA`, `AB`, ... for rows past 26 (1536-well plates have 32 rows).
pub(crate) fn row_letters(row: u32) -> String {
    let mut n = row + 1;
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push(b'A' + r as u8);
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Well name from zero-based row and column under RDML label formats.
pub(crate) fn well_name(row: u32, column: u32, row_label: &str, column_label: &str) -> String {
    let r = if row_label == "123" {
        (row + 1).to_string()
    } else {
        row_letters(row)
    };
    let c = if column_label == "ABC" {
        row_letters(column)
    } else {
        (column + 1).to_string()
    };
    if row_label == "123" && column_label == "123" {
        return format!("{r}-{c}");
    }
    format!("{r}{c}")
}

/// Everything read from one file.
#[derive(Debug, Clone)]
pub(crate) struct QpcrData {
    pub(crate) dialect: Dialect,
    pub(crate) format_version: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) instrument: Instrument,
    pub(crate) operator: Option<String>,
    pub(crate) experimenters: Vec<Person>,
    pub(crate) created_at: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) run_state: Option<String>,
    pub(crate) experiment_type: Option<String>,
    pub(crate) chemistry: Option<String>,
    pub(crate) passive_reference: Option<String>,
    /// Endogenous control target(s) and calibrator sample the vendor analysis used (ΔΔCq).
    pub(crate) reference_targets: Vec<String>,
    pub(crate) calibrator_sample: Option<String>,
    pub(crate) samples: Vec<Sample>,
    pub(crate) targets: Vec<Target>,
    pub(crate) dyes: Vec<Dye>,
    pub(crate) programs: Vec<Program>,
    pub(crate) runs: Vec<Run>,
    /// Vendor-computed standard curves (`.eds` JSON layout), per target.
    pub(crate) standard_curves: Vec<StandardCurve>,
    /// Parts of the file that were missing or could not be read (reported in `info`).
    pub(crate) notes: Vec<String>,
    /// The vendor's metadata, names untouched (for `info --view full`).
    pub(crate) vendor: Value,
    /// Genotyping (allelic discrimination) calls of the first run, as the vendor made them.
    pub(crate) genotypes: Vec<GenotypeCall>,
}

/// One well's genotyping call (`.eds` SDS layout), as the vendor software made it.
#[derive(Debug, Clone, Default)]
pub(crate) struct GenotypeCall {
    /// Zero-based position, row-major.
    pub(crate) position: u32,
    pub(crate) sample: Option<String>,
    pub(crate) marker: Option<String>,
    pub(crate) task: Option<String>,
    /// The file's call code (1, 2, 3, 0, −1).
    pub(crate) code: Option<i64>,
    /// Allele-1 and allele-2 reporter signals, the reference dye signal, the call confidence.
    pub(crate) rn_x: Option<f64>,
    pub(crate) rn_y: Option<f64>,
    pub(crate) reference: Option<f64>,
    pub(crate) confidence: Option<f64>,
    /// `Auto` or `Manual`.
    pub(crate) method: Option<String>,
    /// The marker's allele names (allele 1, allele 2).
    pub(crate) alleles: Option<(String, String)>,
}

impl GenotypeCall {
    /// Our name for the call code (`docs/formats/qpcr.md` § Genotyping).
    pub(crate) fn call(&self) -> String {
        match self.code {
            Some(1) => "allele 1/allele 1".into(),
            Some(2) => "allele 1/allele 2".into(),
            Some(3) => "allele 2/allele 2".into(),
            Some(0) => "negative control".into(),
            Some(-1) => "undetermined".into(),
            Some(c) => format!("code {c}"),
            None => "no call".into(),
        }
    }

    /// The genotype in the marker's allele names (`A/T`), for the three genotype codes.
    pub(crate) fn genotype(&self) -> Option<String> {
        let (a, b) = self.alleles.as_ref()?;
        match self.code? {
            1 => Some(format!("{a}/{a}")),
            2 => Some(format!("{a}/{b}")),
            3 => Some(format!("{b}/{b}")),
            _ => None,
        }
    }
}

/// A standard curve as the vendor software fitted it.
#[derive(Debug, Clone, Default)]
pub(crate) struct StandardCurve {
    pub(crate) target: String,
    pub(crate) dye: Option<String>,
    pub(crate) slope: Option<f64>,
    pub(crate) intercept: Option<f64>,
    pub(crate) r2: Option<f64>,
    /// Percent (100 = doubling every cycle), as vendors report it.
    pub(crate) efficiency_percent: Option<f64>,
}

impl QpcrData {
    pub(crate) fn new(dialect: Dialect) -> Self {
        QpcrData {
            dialect,
            format_version: None,
            name: None,
            description: None,
            instrument: Instrument::default(),
            operator: None,
            experimenters: Vec::new(),
            created_at: None,
            started_at: None,
            ended_at: None,
            run_state: None,
            experiment_type: None,
            chemistry: None,
            passive_reference: None,
            reference_targets: Vec::new(),
            calibrator_sample: None,
            samples: Vec::new(),
            targets: Vec::new(),
            dyes: Vec::new(),
            programs: Vec::new(),
            runs: Vec::new(),
            standard_curves: Vec::new(),
            notes: Vec::new(),
            genotypes: Vec::new(),
            vendor: Value::Null,
        }
    }

    pub(crate) fn target(&self, name: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.name == name)
    }

    pub(crate) fn sample(&self, name: &str) -> Option<&Sample> {
        self.samples.iter().find(|s| s.name == name)
    }

    /// Dyes in order of first use: declared dyes, then target reporters, then signals.
    pub(crate) fn dye_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |d: &str| {
            if !d.is_empty() && !out.iter().any(|x| x == d) {
                out.push(d.to_string());
            }
        };
        for d in &self.dyes {
            push(&d.name);
        }
        for t in &self.targets {
            if let Some(d) = &t.dye {
                push(d);
            }
        }
        for r in &self.runs {
            for rx in &r.reactions {
                for a in &rx.assays {
                    if let Some(d) = &a.dye {
                        push(d);
                    }
                }
                for s in &rx.signals {
                    push(&s.dye);
                }
            }
        }
        out
    }
}

/// Map a vendor task / RDML sample type to our vocabulary.
pub(crate) fn task_name(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "unkn" | "unknown" | "target" => "unknown".into(),
        "std" | "standard" => "standard".into(),
        "ntc" | "no template control" => "ntc".into(),
        "nac" => "nac".into(),
        "ntp" => "ntp".into(),
        "nrt" | "minus rt" | "-rt" => "nrt".into(),
        "pos" | "positive" | "positive control" | "ipc" => "positive".into(),
        "opt" => "optical calibrator".into(),
        "negative" | "negative control" | "neg" => "negative".into(),
        "blocked_ipc" | "blocked ipc" => "blocked ipc".into(),
        "none" | "" => "none".into(),
        other => other.replace('_', " "),
    }
}

/// Parse a number the way vendors write them (`-1`, `NaN`, `Undetermined`, empty → `None`).
pub(crate) fn num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod genotype_tests {
    use super::GenotypeCall;

    #[test]
    fn call_codes() {
        let mut g = GenotypeCall {
            alleles: Some(("A".into(), "T".into())),
            ..GenotypeCall::default()
        };
        for (code, call, gt) in [
            (1, "allele 1/allele 1", Some("A/A")),
            (2, "allele 1/allele 2", Some("A/T")),
            (3, "allele 2/allele 2", Some("T/T")),
            (0, "negative control", None),
            (-1, "undetermined", None),
            (7, "code 7", None),
        ] {
            g.code = Some(code);
            assert_eq!(g.call(), call);
            assert_eq!(g.genotype().as_deref(), gt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_names() {
        assert_eq!(well_name(0, 0, "ABC", "123"), "A1");
        assert_eq!(well_name(15, 23, "ABC", "123"), "P24");
        assert_eq!(well_name(31, 47, "ABC", "123"), "AF48");
        assert_eq!(well_name(1, 2, "123", "123"), "2-3");
        assert_eq!(row_letters(25), "Z");
        assert_eq!(row_letters(26), "AA");
    }

    #[test]
    fn program_cycles_and_anneal() {
        let p = Program {
            stages: vec![
                Stage {
                    kind: "hold".into(),
                    repeats: 1,
                    steps: vec![Step {
                        kind: "temperature".into(),
                        temperature_c: Some(95.0),
                        ..Step::default()
                    }],
                },
                Stage {
                    kind: "cycling".into(),
                    repeats: 40,
                    steps: vec![
                        Step {
                            kind: "temperature".into(),
                            temperature_c: Some(95.0),
                            ..Step::default()
                        },
                        Step {
                            kind: "temperature".into(),
                            temperature_c: Some(60.0),
                            measure: Some("real time".into()),
                            ..Step::default()
                        },
                    ],
                },
            ],
            ..Program::default()
        };
        assert_eq!(p.cycles(), Some(40));
        assert_eq!(p.acquisition_temperature(), Some(60.0));
    }
}
