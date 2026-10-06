# conda-forge pull request

These are recipes for conda-forge/staged-recipes, in the v1 `recipe.yaml` format that
staged-recipes asks new recipes to use (the v0 `meta.yaml` format is deprecated there).

| Directory here | Conda package | Built from |
| --- | --- | --- |
| `python-openreadout/` | `python-openreadout` | PyPI sdist `openreadout-0.1.0.tar.gz`, maturin, abi3 |
| `bioio-openreadout/` | `bioio-openreadout` | PyPI sdist, `noarch: python` |
| `napari-openreadout/` | `napari-openreadout` | PyPI sdist, `noarch: python` |

The Python extension is called `python-openreadout` on conda-forge because the bioconda recipe in
`integrations/bioconda/` uses the name `openreadout` for the command-line tool. bioconda and
conda-forge are used together, and staged-recipes rejects a name that already exists in bioconda
(and bioconda's linter rejects one that exists in conda-forge). Inside Python nothing changes:
`pip` and `import` still see `openreadout`.

To submit, fork conda-forge/staged-recipes, copy the three directories to `recipes/`, and open one
pull request with the title and body below. Then comment
`@conda-forge-admin, please ping team` or ask `@conda-forge/help-rust` and
`@conda-forge/help-python` for review.

## Title

Add python-openreadout, bioio-openreadout and napari-openreadout

## Body

This adds three packages from [OpenReadout](https://github.com/openreadout/openreadout), which
reads raw files from life-science instruments (microscopes, flow cytometers, mass spectrometers,
NMR and more) without vendor software.

- `python-openreadout` is the Python extension (`import openreadout`), written in Rust with PyO3
  and built with maturin from the PyPI sdist. It uses the stable ABI (`abi3-py310`), so it
  follows the python-abi3 example: one build per platform, tested with `abi3audit`. The package
  is named with the `python-` prefix because bioconda will carry the command-line tool as
  `openreadout`.
- `bioio-openreadout` and `napari-openreadout` are pure-Python reader plugins for bioio and napari
  that depend on it.

Rust dependency licenses are collected with `cargo-bundle-licenses` into `THIRDPARTY.yml` and
packaged with the project's `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE`.

I maintain the upstream project.

Checklist

- [x] Title of this PR is meaningful: e.g. "Adding my_nifty_package", not "updated recipe".
- [x] Recipe uses the v1 `recipe.yaml` format, or this PR explains the exceptional requirement for deprecated v0.
- [x] License file is packaged (see the [v1 example recipe](https://github.com/conda-forge/staged-recipes/blob/main/recipes/example-v1/recipe.yaml) for an example).
- [x] Source is from official source.
- [x] Package does not vendor other packages. (If a package uses the source of another package, they should be separate packages or the licenses of all packages need to be packaged).
- [x] If static libraries are linked in, the license of the static library is packaged.
- [x] Package does not ship static libraries. If static libraries are needed, [follow CFEP-18](https://github.com/conda-forge/cfep/blob/main/cfep-18.md).
- [x] Build number is 0.
- [x] A tarball (`url`) rather than a repo (e.g. `git_url`) is used in your recipe (see [here](https://conda-forge.org/docs/maintainer/adding_pkgs.html#build-from-tarballs-not-repos) for more details).
- [ ] GitHub users listed in the maintainer section have posted a comment confirming they are willing to be listed there.
- [x] When in trouble, please check our [knowledge base documentation](https://conda-forge.org/docs/maintainer/knowledge_base.html) before pinging a team.

## Local verification (2026-10-06, macOS 26 on Apple silicon)

These steps aren't part of the pull request. They record what was checked before submitting.

- The `sha256` values are those PyPI publishes for the three sdists, and match the downloaded
  files.
- `conda-smithy recipe-lint --conda-forge` passes for all three recipes. It doesn't run the
  staged-recipes checks for existing names, which is why the Python package is named
  `python-openreadout` (see above).
- rattler-build 0.76.1 built `python-openreadout` natively for osx-arm64 with the current
  conda-forge pinning file (Rust 1.98.1, clang 21, macOS 11.0 target, Python 3.11 as
  `python_min`). Only the `is_python_min` variant is built, and the package is marked
  version-independent. The build took 20 minutes with 2 cargo jobs. The tests passed: import and
  `pip check` on Python 3.11 and 3.14, the version and format-list check, and `abi3audit` (no ABI
  violations). The package contains `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE` and
  `THIRDPARTY.yml`, and the extension links only `libSystem`.
- `bioio-openreadout` and `napari-openreadout` built as `noarch: python` against that local
  `python-openreadout`. Import and `pip check` passed on Python 3.11 and the latest Python, and
  the packaged pytest suites passed (bioio: 5 passed, 19 skipped; napari: 3 passed, 4 skipped;
  the skipped tests need the project's test files, which the sdists don't include).
- In one environment with all three packages and the bioconda `openreadout` CLI built locally,
  `openreadout self doctor --write-fixtures` wrote a TIFF that `openreadout.info`, bioio's
  `BioImage` with the plugin, and the napari reader all opened.
- For the local build only, the `python-openreadout` script also set `CARGO_BUILD_JOBS=2` and
  `HOME`, because this machine has a cargo wrapper in `~/.cargo/config.toml`. The recipe here
  doesn't have those settings.
- Not tested locally: Linux and Windows builds, and cross-compiling osx-arm64 from osx-64 as
  conda-forge's CI does.
