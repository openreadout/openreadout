# Scope and roadmap

OpenReadout reads the raw files that lab instruments write and turns them into JSON, analyses and open formats. [Formats](../formats/index.md) lists each supported format and its known gaps. Closing those gaps is the near-term work.

## What comes next

- **More versions of the formats already read.** Most work goes into new variants: a new vendor software release, instrument generation or codec. If OpenReadout fails on one of your files, run `openreadout report FILE` and attach the bundle to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml).
- **New formats.** We pick them by how many labs depend on them and whether there are public sample files to test against. [Bench instruments without open readers](bench-instruments-survey.md) lists candidates. Requests are welcome as issues.
- **Writers for experiment set-up files**, such as acquisition worklists and analysis workspaces. Not for raw data.

## Out of scope

- Rewriting raw acquisition files in place. The raw file is the primary record.
- Encrypted or deliberately obfuscated formats, such as Sciex `.wiff2` (see the [clean-room policy](clean-room.md)).
- Sequencing and clinical formats that are already well served, such as FASTQ, BAM, VCF and DICOM.
