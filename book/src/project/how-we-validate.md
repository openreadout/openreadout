# How we validate

Most lab-instrument formats have no public specification, so a reader needs evidence that it reads real files correctly.

## Readers written in a clean room

We work out each format from files we are allowed to use and from the published documentation of permissively licensed readers. We don't use vendor SDKs, headers, DLLs or non-public specifications, and we don't read the source code of copyleft readers while writing a parser. Each format has a provenance log that records what we inferred and from which files. The rules are in the [clean-room policy](clean-room.md).

## Checked against independent readers

The readers are tested on public files from real instruments, deposited by labs in open repositories. The test corpus lists each file with its source, license and checksum ([`corpus/manifest.toml`](../../../corpus/manifest.toml)).

For each file, we compare OpenReadout's output with another program that opens the same file: an independent reader such as czifile, nd2, Bio-Formats, FlowIO or pyABF, an export made with the vendor's own software, or results the vendor software stored in the file. We run these programs as black boxes and keep only their output. Decoded pixels must match exactly (lossy codecs such as JPEG get a small tolerance), and sizes, channels and metadata must agree. When two readers disagree, a third one decides, and we write down which reader was right and why. We don't adjust OpenReadout's output just to make it agree with another reader.

From this evidence, each reader gets a confidence level of high, medium or low. A fixed rule computes the level from how many files and depositors confirm the reader. Nobody sets it by hand. The [format list](../formats/index.md) shows the level of every format.

## What validated means for your file

A reader can pass all its tests and still misread a variant it hasn't seen, such as a new software version or an unusual codec. So `info` and `check` say how well that particular file is covered:

- **Validated**: other files of the same variant were read correctly and confirmed by an independent reader.
- **Partially validated**: the values are decoded the same way as on confirmed files, but something is less certain. For example, the writer version is new, or a value was assumed.
- **Unvalidated**: some output depends on something no independent reader has confirmed, so its values may be wrong.

With `--strict`, OpenReadout refuses outputs that aren't validated and withholds single fields that no independent reader has checked. [Assurance and strict mode](../reference/assurance.md) explains the details.

## Files no reader was developed on

A reader that works on the files it was built from may still fail on the next lab's files. To measure that, we keep a held-out set: public files from sources that no reader was developed or debugged on. We only run the readers on them to see whether they generalize. When a held-out file shows a problem, we fix it on a new public file that shows the same problem, so the held-out set stays a fair test.

## When a file doesn't work

1. Run `openreadout info --view format FILE`. Exit code 3 means the format isn't supported. Exit code 6 means the format is known but the file uses a feature that isn't decoded yet, and the `hint` says which.
2. Run `openreadout report FILE`. It writes a diagnostic bundle that you can review before sharing.
3. Attach the bundle to a [new-variant issue](https://github.com/openreadout/openreadout/issues/new?template=new-variant.yml) with the instrument, the acquisition software and its version.

If the file can be shared under an open license, it can join the test corpus. That is the fastest way to get it fixed, and files from instruments or software versions the corpus doesn't have yet help the most.

This is not validation in the regulatory sense (IQ/OQ/PQ under GxP). The [FAQ](faq.md#can-i-use-it-in-a-regulated-gxp-21-cfr-part-11-lab) says what that means for a regulated lab.
