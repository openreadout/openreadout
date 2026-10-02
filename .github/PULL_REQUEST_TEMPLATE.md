## What and why

<!-- One or two sentences. Link the issue: "Fixes #123". -->

## How it was tested

<!-- Commands you ran, corpus files you used, before/after output. -->

## Checklist

- [ ] Every commit is signed off (`git commit -s`, Developer Certificate of Origin); the DCO check enforces it.
- [ ] **Clean room:** I did not consult any vendor SDK, header, DLL, decompiled binary, or non-public / NDA specification, nor the source code of a GPL/LGPL reader, while writing this change. See `docs/legal/clean-room-policy.md`.
- [ ] Provenance log updated: if parsing logic changed, `docs/provenance/<fmt>.md` has a dated entry (corpus files used, prior art consulted with URL + license, what was inferred from what). Tick also if no parsing logic changed.
- [ ] New public identifiers in a format crate are in the vocabulary table of `docs/formats/<fmt>.md`, and `cargo xtask vocab-check` passes.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` pass.
- [ ] Reader changes: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus` passes on the smoke tier; new disagreements are arbitrated and recorded in `corpus/manifest.toml`.
- [ ] JSON output changed? `cargo xtask schema gen` was run and `docs/schema/` is committed; no existing field renamed or removed without a `schema_version` bump.
- [ ] User-visible change? `CHANGELOG.md` (Unreleased), the book (`book/src/`) and, if agents need to know, `skills/openreadout/` (then `cargo xtask skill-parity --fix`) are updated.
- [ ] AI-assisted? I reviewed every line, and no identifier came from anywhere but this repository's docs and the files themselves.
