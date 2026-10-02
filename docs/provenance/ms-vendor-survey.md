# Survey — public vendor MS raw data with depositor conversions (Waters, Agilent MassHunter, Sciex)

Rules: `docs/legal/clean-room-policy.md`. This log records where validation pairs for the next mass-spectrometry readers can come from. No file listed here has been downloaded, opened or added to `corpus/manifest.toml` yet, and no parsing logic was written from it. Each format's own provenance log (`docs/provenance/<fmt>.md`) starts when its first file is used.

## 2026-09-23 — MetaboLights listing scan (Richard Zimring with Claude as assistant)

**Method.** The HTTP directory listings `https://ftp.ebi.ac.uk/pub/databases/metabolights/studies/public/<MTBLS>/FILES/` (plus up to eight sub-directories per study whose names suggest raw or converted data, e.g. `RAW_FILES/`, `DERIVED_FILES/`) were read for 2,015 of the 3,452 public studies (MTBLS1 onwards; the scan stopped early for time). A *pair* is a vendor file or directory and a depositor-made `.mzML`/`.mzXML` with the same stem. Only names and listed sizes were read, no file contents.

**Licences.** MetaboLights data are released under the EMBL-EBI Terms of Use (no restrictions on use or redistribution); some newer studies declare CC0 in their metadata. Each file's licence must be checked and recorded when it enters the manifest, as for the Thermo entries.

### Agilent MassHunter `.d` (zipped directories)

| study | pairs | smallest pair (vendor, conversion) |
| --- | --- | --- |
| MTBLS449 | 80 | `13047CHQ_0001_A1.d.zip` 1.1 MB, `13047CHQ_0001_A1.mzML` 857 KB |
| MTBLS243 | 22 | `03_D24062013T1259_1399CBU_01QC_A3.d.zip` 1.2 MB, `….mzML.zip` 628 KB |
| MTBLS599 | 14 | `AZA-26-50.d.zip` 3.1 MB, `AZA-26-50.mzXML` 2.1 MB |
| MTBLS874 | 210 | `BDV10076M3.d.zip` 4.8 MB, `BDV10076M3.mzML` 7.6 MB |
| MTBLS11870 | 19 | `QC_1_NEG.d.zip` 6.7 MB, `QC_1_NEG.mzML` 5.9 MB |
| MTBLS1334 | 55 | `STD_neg_MSMS_1min0205.d.zip` 7.4 MB, `….mzML` 3.1 MB (MS/MS in the name) |
| MTBLS968 | 54 | `Z-3-2-1-B.D.zip` 8.4 MB, `Z-3-2-1-B.mzML` 52 MB |
| MTBLS1375 | 157 | `1_LTR_1_A-1.d.zip` 14 MB, `1_LTR_1_A-1.mzML` 2.1 MB |
| MTBLS7386 / 7384 / 7390 | 44 / 218 / 28 | 16–20 MB `.d.zip` with 14–17 MB mzML (2024–2025 acquisitions) |
| MTBLS10740 | 692 | `240409DA_PilotStudy_HILIC_neg_085.d.zip` 20 MB, mzML 43 MB |
| MTBLS4722, 2313, 6740, 5163, 5196, 1636 | 4–50 | 31–66 MB `.d.zip` (MTBLS6740: auto MS/MS — **correction 2026-09-23:** its `.d` directories are Bruker BAF, not MassHunter; see `docs/provenance/agilent-masshunter.md`) |

Agilent is the best-supplied of the three: small pairs across a decade of acquisition-software versions, profile and MS/MS runs.

### Waters MassLynx `.raw` (zipped directories)

| study | pairs | smallest pair | note |
| --- | --- | --- | --- |
| MTBLS225 | 60 | `TQS-RLN-20140623-056.raw.zip` 190 KB, `.mzXML` 4.9 MB | Xevo TQ-S (name), likely MRM |
| MTBLS2266 | 1 | `RAW_FILES/QC142.raw.zip` 22 KB, `DERIVED_FILES/QC142.mzML` 285 MB | the zip is too small to hold the run; probably incomplete upload |
| MTBLS7290 | 185 | `RAW_FILES/DIME__SCFA240_20220530_SSCFA_001_neg_Blank.raw.zip` 31 MB, mzML 1.5 MB | best first full-scan candidate |
| MTBLS3128 | 2 | `FMTB_ROS_POS_EST1_BRANCO2.raw.zip` 166 MB, mzML 10 KB | the mzML is too small to be the run |
| MTBLS694 | 213 | `PipelineTesting_RPOS_ToF10_B1SRD51.raw.zip` 888 MB, mzML 110 MB | ToF full scan; large |
| MTBLS6081 | 8 | 4.1 GB `.raw.zip`, 8.3 GB mzML | out of the size budget |

Studies with `.raw` *files* and mzML (MTBLS20, 404, 755, 797, 805, …) are Thermo, not Waters.

### Sciex `.wiff` + `.wiff.scan`

A `.wiff` without its `.wiff.scan` holds no spectra, so only studies that uploaded both are usable:

| study | smallest pair | note |
| --- | --- | --- |
| MTBLS6084 | `SL_ST_Blank2.wiff` 1.2 MB + `.wiff.scan` 234 KB, `SL_ST_Blank2.mzML` 2.1 MB | small; best first candidate |
| MTBLS11360 | `RAW_FILES/Col-0-1_N.wiff` 1.8 MB + `.wiff.scan` ~36 MB, `DERIVED_FILES/….mzML` 117 MB | |
| MTBLS9657 | `RAW_FILES/Control 1.wiff` 2.5 MB + `.wiff.scan` ~42 MB, `.mzXML` 447 MB | |
| MTBLS10498 | `RAW_FILES/17_20230915_qc1-pos.wiff` 3.3 MB + `.wiff.scan` 65 MB, `.mzXML` 113 MB | |
| MTBLS297, MTBLS4618, MTBLS4708 | `.wiff` only in the listing (MTBLS4708 has `wiffscan/` sub-directories not listed) | |

No `.wiff2` file appeared in the 2,015 listings.

### Not yet surveyed

MetaboLights studies beyond the 2,015 listed; PRIDE (Sciex TripleTOF and Waters SYNAPT deposits, via `/pride/ws/archive/v3/projects/<PXD>/files`); MassIVE; Metabolomics Workbench; Panorama Public. Bruker `.d` TSF and `.baf` were not searched.

## 2026-09-24 — Agilent OpenLab CDS (`.dx`, `.rslt`/ACAML): no usable public files

Searched for OpenLab CDS data containers with a licence that permits use and redistribution: Zenodo (full-text queries "OpenLab CDS", "OpenLab" + "dx", "ACAML"), GitHub code search (`*.dx` with ACAML), MetaboLights and PRIDE listings. No file was found that meets clean-room policy rules 1 and 10. No reader was written. What would unblock it: one `.dx` file (with, ideally, a vendor CSV/ANDI export of the same injection) deposited under CC-BY/CC0, or donated with written permission.

## 2026-09-24 — Agilent OpenLab CDS: usable files found (update)

A second search found two licensed sources, now in the corpus: Zenodo record 14316687 (Katzenburg et al., CC-BY-4.0; 54 GC-FID injections as `.dx` + `.rx` with the vendor's CSV and PDF reports) and the OpenLab CDS test result sets of allotropy (Benchling, MIT; an LC with DAD/FLD and instrument curves). The reader (`openlab-cds`) and its derivation are in `docs/formats/openlab-cds.md` and `docs/provenance/openlab-cds.md`. Still missing: an OpenLab LC-MS or DAD-spectra (`.UV`) injection under a redistributable licence.
