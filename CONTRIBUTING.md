# Contributing

Thank you for helping make lab data readable. Bug reports, files that fail, documentation fixes and code are all welcome.

## Clean room

Parsers are written only from files we may hold and from permissively licensed, published prior art. Never consult a vendor SDK, header, DLL or non-public specification, and never read the source of a GPL or LGPL reader while writing a parser. The full rules are in the [clean-room policy](docs/legal/clean-room-policy.md); read it before you touch a parser. Pull requests that break it are closed without merge.

## Sign-off

Every commit needs a [Developer Certificate of Origin](https://developercertificate.org/) sign-off. `git commit -s` adds the line:

```
Signed-off-by: Your Name <you@example.com>
```

By signing off you certify that you wrote the change or have the right to submit it under this project's license (MIT OR Apache-2.0).

## Build and test

```bash
cargo build
cargo nextest run                          # unit and golden-output tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo xtask vocab-check                    # every public field name is documented
```

The corpus tests compare each reader with independent readers on public files:

```bash
cargo xtask corpus fetch --tier smoke      # about 1 GB of public test files
(cd oracle && uv sync)                     # the reference readers, in Python
cargo test -p openreadout-corpus-tests --features corpus --profile corpus
```

Commit messages use the Angular style: `feat(nd2): ...`, `fix(czi): ...`, `docs: ...`.

## Maintainer documentation

- [docs/maintaining.md](docs/maintaining.md): adding a format, turning a failing file into a supported variant, the release cadence.
- [docs/architecture.md](docs/architecture.md): how the crates fit together.
- [docs/formats/](docs/formats) and [docs/provenance/](docs/provenance): what each format holds, and how we learned it.
- `crates/<crate>/MAINTAINING.md`: a guide for each reader.
- [docs/release-process.md](docs/release-process.md): cutting a release.

AI-assisted contributions are welcome if you have reviewed every line and every identifier comes from this repository's docs or from the file itself.
