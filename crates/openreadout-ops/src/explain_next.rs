//! `info --view explain`'s analysis suggestions: the next command that answers a scientist's usual
//! question about this kind of data (intensities, peak areas, APs, NMR peaks, Cq, concentrations),
//! so the suggestions route to the analysis commands instead of only to exports.

use openreadout_core::model::FileInfo;

/// Analysis commands worth running next on `info` (`file` is already shell-quoted), most
/// useful first.
pub fn analysis_commands(info: &FileInfo, file: &str) -> Vec<String> {
    let mut out = Vec::new();
    let family = info.format.family.as_str();
    let id = info.format.id.as_str();
    let is_plate = info.images.iter().any(|im| im.extra.contains_key("well"));
    if !info.images.is_empty() && !is_plate {
        let rgb = info.images.iter().any(|im| im.samples_per_pixel >= 3);
        out.push(if rgb {
            format!(
                "openreadout stats {file} --no-planes --json   # intensity statistics; channels[].components[] = red, green, blue"
            )
        } else {
            format!(
                "openreadout stats {file} --no-planes --json   # intensity per channel: mean, percentiles, saturated_count"
            )
        });
    }
    if !info.spectra.is_empty() {
        out.push(format!(
            "openreadout analyze chromatogram {file} --tic --json   # the TIC and its apex; --mz M --ppm 10 for an extracted-ion chromatogram"
        ));
        out.push(format!(
            "openreadout analyze peaks {file} --json   # integrated peaks (RT, area, height, area %)"
        ));
    }
    if !info.traces.is_empty() {
        match family {
            "electrophysiology" => {
                let fast = info.traces.iter().any(|t| t.sample_rate_hz >= 20_000.0);
                if id.starts_with("abf") || id == "atf" {
                    out.push(format!(
                        "openreadout analyze ephys-features {file} --json   # action potentials per sweep, rheobase, input resistance, tau"
                    ));
                } else if fast {
                    out.push(format!(
                        "openreadout analyze spikes {file} --json   # extracellular spike counts and rates per channel (pick the broadband --trace)"
                    ));
                }
                out.push(format!(
                    "openreadout trace {file} --tidy --json   # min, max, mean and time of the maximum for every sweep and channel"
                ));
            }
            "nmr" => out.push(format!(
                "openreadout analyze nmr-peaks {file} --json   # peak list and the tallest peak (ppm); --integrate A:B for integrals"
            )),
            "chromatography" => {
                out.push(format!(
                    "openreadout analyze peaks {file} --json   # integrated peaks (RT, area, height, area %, S/N)"
                ));
                out.push(format!(
                    "openreadout trace {file} --json   # argmax_axis_value = retention time (min) of the highest point"
                ));
            }
            "spectroscopy" => out.push(format!(
                "openreadout trace {file} --json   # argmax_axis_value = position of the strongest band"
            )),
            "qpcr" => out.push(format!(
                "openreadout analyze qpcr {file} --json   # Cq, Tm and task per well and target; --ddcq for fold changes"
            )),
            "mass-spectrometry" => out.push(format!(
                "openreadout analyze peaks {file} --trace 0 --json   # peaks of the first detector trace (UV/DAD)"
            )),
            _ => {}
        }
    }
    if !info.tables.is_empty() {
        match family {
            "plate-reader" => {
                out.push(format!(
                    "openreadout analyze assay wells {file} --json   # per-well values with roles, blanks, replicate CV"
                ));
                out.push(format!(
                    "openreadout analyze assay curve {file} --json   # standard curve and back-calculated concentrations"
                ));
            }
            "flow-cytometry" => out.push(format!(
                "openreadout table {file} --tidy --json   # events, median, mean and sd per parameter"
            )),
            "qpcr" if info.traces.is_empty() => out.push(format!(
                "openreadout analyze qpcr {file} --json   # Cq, Tm and task per well and target"
            )),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::PixelType;
    use openreadout_core::model::{FormatDescriptor, ImageInfo, TraceInfo};
    use openreadout_core::provenance::Confidence;

    fn info(family: &str, id: &str) -> FileInfo {
        FileInfo {
            path: "x".into(),
            size_bytes: 1,
            format: FormatDescriptor {
                id: id.into(),
                name: id.into(),
                vendor: String::new(),
                extensions: vec![],
                family: family.into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::High,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            plane_count: 0,
            notes: vec![],
        }
    }

    #[test]
    fn suggestions_follow_the_family() {
        let mut abf = info("electrophysiology", "abf");
        abf.traces.push(TraceInfo::default());
        let s = analysis_commands(&abf, "x.abf");
        assert!(
            s[0].contains("ephys-features") && s[1].contains("--tidy"),
            "{s:?}"
        );
        let mut nmr = info("nmr", "bruker-nmr");
        nmr.traces.push(TraceInfo::default());
        assert!(analysis_commands(&nmr, "d")[0].contains("nmr-peaks"));
        let mut img = info("microscopy", "czi");
        let mut im = ImageInfo::new(0, 4, 4, PixelType::Uint8);
        im.samples_per_pixel = 3;
        img.images.push(im);
        assert!(analysis_commands(&img, "a.czi")[0].contains("components"));
        let mut plate = info("plate-reader", "plate");
        plate.tables.push(Default::default());
        assert!(analysis_commands(&plate, "p.csv")[0].contains("assay wells"));
    }
}
