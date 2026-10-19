# DNA sequencers

OpenReadout reads what DNA sequencers write, without the vendor's software:

| what | format id | page |
| --- | --- | --- |
| Illumina run folders: `RunInfo.xml`, `RunParameters.xml`, InterOp metrics, BCL and CBCL base calls | `illumina-run` | [Illumina run folders](illumina-run.md) |
| Oxford Nanopore raw signal | `pod5`, `fast5` | [POD5 and FAST5](nanopore.md) |
| Reads as sequencers and basecallers hand them on (Illumina, PacBio HiFi, Oxford Nanopore) | `fastq`, `bam` | [FASTQ and BAM](sequence-reads.md) |
| Reference-compressed alignments (CRAM 2.0 to 3.1); bases need the reference, embedded or as a FASTA beside the file | `cram` | [CRAM](cram.md) |

![Run quality of an Illumina NovaSeq 6000 run, drawn by openreadout preview](../img/gallery/run-quality.png)

## A run folder

`info` reads the run's layout and the InterOp metrics, never the base calls, so it answers in
well under a second on a run of terabytes:

```console
$ openreadout info 200624_A00834_0183_BHMTFYDRXX --json | jq '.data.tables[0].extra.totals'
{
  "yield_gbp": 56.3668,
  "percent_q30": 94.281,
  "reads_m": 1276.67,
  "reads_pf_m": 1063.38,
  "percent_pf": 83.293,
  "lanes": 2,
  "cycles": 56,
  "read_structure": "36T10B10B"
}
```

Table 0 is the run summary per read and lane (density, % passing filter, reads, % ≥ Q30,
yield, error rate, phasing, % occupied), table 1 the metrics per lane and cycle, table 2 per
tile, table 3 the Q-score histogram and table 4 the index counts. Trace 0 holds the quality by
cycle with one sweep per lane, so `openreadout trace RUN --sweep 1` is lane 2 and
`openreadout plot` draws it:

```console
$ openreadout plot '{"data": {"file": "RUN", "y": "percent_q30", "sweep": [0, 1]}}' -o q30.png
```

`openreadout preview RUN` draws the picture above. `openreadout export RUN --format fastq`
writes gzip FASTQ per lane and read from the base calls (`--rows 0-9999` for the first 10,000
clusters of each lane), and `openreadout check RUN` decodes every base call.

## Nanopore reads

Every read is one sweep of trace 0, in picoamperes:

```console
$ openreadout trace HG002.pod5 --sweep 3 --max-samples 5
$ openreadout preview HG002.pod5 --sweep 3
```

![A nanopore read's raw signal](../img/gallery/squiggle.png)

Table 0 lists the reads (channel, well, start, duration, end reason, open-pore level), and
`openreadout export HG002.pod5 --format parquet --trace 0` writes every read's samples to Parquet
(a `sweep` column numbers the reads; without `--trace` it writes the reads table).

## Read files

`info` streams the first 32 MB of a FASTQ or BAM file and reports read counts, the N50, %
bases ≥ Q30 and GC; it says when counts are estimated, and `check` reads every record.

![Read lengths and qualities of a HiFi FASTQ file](../img/gallery/read-lengths.png)

## Across runs

`batch` compares runs: one row of instrument, run and sample facts per run folder, or the
columns of the run summary per run:

```console
$ openreadout batch info 'runs/*/' -o runs.csv
$ openreadout batch table 'runs/*/' --set table=0 --set 'parameters=["percent_q30", "yield"]' -o q30.csv
```

## Not read

Ion Torrent (`.wells`, `.dat`), MGI and Element Biosciences run folders, PacBio raw movie and
`.baz` files, Sanger and fragment-analysis traces are not read yet. CRAM is not read; samtools
reads it.
