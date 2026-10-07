"""Snakemake wrapper for `openreadout export`: a raw instrument file to an open format.

The target format comes from `params.format`, else from the output's extension. The output is
written under a temporary name, read back and verified by openreadout, then renamed into place;
the optional `report` output keeps the JSON export report (`verified`, sizes, planes or rows).
"""

__author__ = "Richard Zimring"
__copyright__ = "Copyright 2026, The OpenReadout Authors"
__email__ = "openreadout@gmail.com"
__license__ = "MIT"

from snakemake.shell import shell

# `snakemake` (inputs, outputs, params, log) is injected by Snakemake when it runs the wrapper.
# ruff: noqa: F821

EXTENSIONS = [
    (".ome.tiff", "ome-tiff"),
    (".ome.tif", "ome-tiff"),
    (".ome.zarr", "ome-zarr"),
    (".zarr", "ome-zarr"),
    (".mzml", "mzml"),
    (".parquet", "parquet"),
    (".arrow", "arrow"),
    (".feather", "arrow"),
    (".csv", "csv"),
    (".nwb", "nwb"),
    (".jdx", "jcamp"),
    (".dx", "jcamp"),
    (".rdml", "rdml"),
    (".asm.json", "asm"),
]


def target_format(output: str) -> str:
    lower = output.lower().rstrip("/")
    for ext, fmt in EXTENSIONS:
        if lower.endswith(ext):
            return fmt
    raise ValueError(
        f"openreadout export: cannot tell the format from {output!r}; "
        "set params.format (ome-tiff, ome-zarr, mzml, csv, parquet, arrow, nwb, jcamp, rdml, asm)"
    )


extra = snakemake.params.get("extra", "")
output = snakemake.output.get("output", snakemake.output[0])
fmt = snakemake.params.get("format") or target_format(output)
report = snakemake.output.get("report")
log = snakemake.log_fmt_shell(stdout=report is None, stderr=True)

if report:
    shell(
        "openreadout export {snakemake.input[0]:q} --format {fmt} -o {output:q} {extra} --json "
        "> {report:q} {log}"
    )
else:
    shell("openreadout export {snakemake.input[0]:q} --format {fmt} -o {output:q} {extra} {log}")
