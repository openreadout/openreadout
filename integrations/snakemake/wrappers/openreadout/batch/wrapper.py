"""Snakemake wrapper for OpenReadout batch tables: one tidy table over many instrument files,
joined to a sample sheet or plate layout, optionally summarized by group.

    openreadout <measure> FILES... --tidy [--sample-sheet SHEET] EXTRA -o TABLE   (measure gate: analyze gate)
    openreadout batch summarize TABLE SUMMARIZE -o SUMMARY    (when output.summary is declared)
"""

__author__ = "Richard Zimring"
__copyright__ = "Copyright 2026, The OpenReadout Authors"
__email__ = "openreadout@gmail.com"
__license__ = "MIT"

from snakemake.shell import shell

# `snakemake` (inputs, outputs, params, log) is injected by Snakemake when it runs the wrapper.
# ruff: noqa: F821

MEASURES = ("stats", "trace", "table", "gate", "info")

measure = snakemake.params.get("measure", "table")
if measure not in MEASURES:
    raise ValueError(f"params.measure must be one of {', '.join(MEASURES)}, not {measure!r}")
extra = snakemake.params.get("extra", "")
summarize = snakemake.params.get("summarize", "")

sheet = snakemake.input.get("sample_sheet")
files = snakemake.input.get("files")
if files is None:
    files = [f for f in snakemake.input if f != sheet]
elif isinstance(files, str):
    files = [files]
if not files:
    raise ValueError("no input files: declare them as input.files (or unnamed inputs)")

table = snakemake.output.get("table", snakemake.output[0])
summary = snakemake.output.get("summary")
if summary and not summarize:
    raise ValueError("output.summary needs params.summarize, e.g. '--by condition --value median'")
sheet_opt = f"--sample-sheet {sheet!r}" if sheet else ""
log = snakemake.log_fmt_shell(stdout=True, stderr=True)

command = "analyze gate" if measure == "gate" else measure
shell("openreadout {command} {files:q} --tidy {sheet_opt} {extra} -o {table:q} {log}")
if summary:
    log_append = snakemake.log_fmt_shell(stdout=True, stderr=True, append=True)
    shell("openreadout batch summarize {table:q} {summarize} -o {summary:q} {log_append}")
