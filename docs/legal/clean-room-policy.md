# Clean-room policy

OpenReadout reads proprietary instrument file formats. Reverse engineering a file format for interoperability is lawful in the United States (Sega v. Accolade, Sony v. Connectix, DMCA §1201(f)) and explicitly protected in the EU (Software Directive, Art. 6). The legal risk is entirely in *how* the work is done. Every rule below exists so that no vendor ever has anything to file on, and so that "how was each reader derived?" always has a short, documented answer

This document describes the project's practice; it is not legal advice.

## The rules

1. **Derive only from files we legitimately hold and from permissively licensed, published prior art.**
   Every input to a parser is one of: a corpus file listed in `corpus/manifest.toml` with its license recorded; a hex dump of such a file; a differential comparison between two such files; the documentation or source of a BSD/MIT-licensed reader (listed in `NOTICE`); or a vendor's *public* documentation page.

2. **Never open vendor-restricted material.**
   That means vendor SDKs, header files, DLLs and decompiled binaries, and any document that is marked confidential or restricted, was obtained under an NDA, a registration wall or a license that limits its use, or was not published by its owner. A document being easy to find online does not make it public. If in doubt, do not open it. If someone sends you such material, do not open it.

3. **Copyleft projects are oracles, not sources.**
   GPL and LGPL readers (Bio-Formats, libCZI, pylibCZIrw, bioio-czi, bioio-lif, readlif) may be *run* to produce ground truth. Their *source code* is not consulted while writing a parser. Their documentation web pages may be read. If a copyleft project's source must be consulted to explain a behavioral difference, the person doing so files only a black-box bug report ("file X, plane 3: expected 512×512 u16, got …"); they do not write the fix.

4. **Skip anything encrypted or deliberately obfuscated.** Sciex `.wiff2` is the known case. Defeating a technical protection measure moves the project into DMCA anti-circumvention territory.

5. **Name every field in our own vocabulary.** Each format's `docs/formats/<fmt>.md` has a vocabulary table. Every public field and enum variant in a format crate must appear there, and `cargo xtask vocab-check` enforces it. This is the mechanical guard against an AI assistant emitting a vendor-internal identifier it may have seen in training.

6. **Provenance is logged before code is written.** `docs/provenance/<fmt>.md` records, with dates: which corpus files were used, which prior art was consulted (URL and license), and what was inferred from what. It is the evidence that makes any later dispute short.

7. **Vendor software is used only to make files, never opened up.** Read the EULA before installing. Producing files with it is ordinary use. If an EULA bars building competing tools, a collaborator at a core facility produces the files instead.

8. **Raw acquisition data is immutable.** The tool never rewrites a raw file in place. Every write is to a new file, verified by reading it back. This is both a data-integrity rule for regulated labs and a liability rule for us.

9. **Contributions.** Every commit carries a Developer Certificate of Origin sign-off (`git commit -s`). Pull requests that reference a vendor SDK, header, or non-public specification are closed without merge. Contributor-supplied infringing code is our distribution, so review for it.

10. **Test corpus.** Only files with a recorded, redistribution-compatible license, or files donated with explicit permission, go in the manifest. Anything medical is anonymized before it touches disk. Vendor demo datasets are not redistributed.

11. **Held-out files are never inputs to a parser.** Files on the `heldout` tier of the manifest measure whether the readers generalize; they are not among the corpus files rule 1 allows as parser inputs. No hex dump, differential comparison or oracle reading of a held-out file informs a parser, and no provenance log cites one (`cargo xtask heldout-check`). A failure found on a held-out file is fixed on a new file. Details: `docs/benchmark/heldout.md`.

## Exposure

A contributor (human or agent session) who has read excluded material for a format does not write that format's parser; someone who has not read it does.

## What we do if a takedown arrives

Do not pull the repository reflexively. GitHub's counter-notice process, its Developer Defense Fund for §1201 claims, the EFF, and the Software Freedom Conservancy exist for exactly this. A clean provenance log is what turns a takedown into a reinstatement.
