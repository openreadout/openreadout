# Write the methods paragraph

When you write up an experiment, the acquisition settings you need (objective, channels, pixel size, z-step, instrument and software) are already in the raw file. `info --view explain` reads them from the headers and writes them out in sentences, so you can check them against your notes and copy them into your methods section.

## Run it

```text
$ openreadout info zstack.czi --view explain
A Zeiss CZI microscopy file (format version 1.0), 65.9 KiB, holding one image. It is a 2-channel ...

Experiment: measured by fluorescence microscopy (FBbi:00000246) (detector PMT + Other, immersion Other, objective EC Plan-Neofluar 10x/0.30 M27, objective magnification 10, objective na 0.3, pixel size 1.66 µm, z step 1 µm); operator m1swg.

The image is 512 × 1 pixels, a field of view of 850 µm × 1.66 µm at 1.66 µm per pixel; 21 focal planes 1 µm apart (a depth of 20 µm); 2 channels; pixels are 16-bit values (0 to 65535), of which ...

Channels: 0 “Ch1” (excited at 561 nm, orange light at 591 nm, shown in white (grey scale)); 1 “ChS1” (excited at 561 nm, green light at 561 nm, ...

Imaged through an EC Plan-Neofluar 10x/0.30 M27 objective (10× magnification, numerical aperture 0.3, other immersion), on a Carl Zeiss Microscopy LSM 510, AxioObserver instrument, ...
...
Caveats:
  - The file does not record when the images were acquired.
  ...
Next steps:
  openreadout stats zstack.czi --no-planes --json   # intensity per channel: mean, percentiles, ...
  ...
```

`zstack.czi` is a copy of the public test file [`fuzz/corpus/whole_czi/zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging.czi`](../../../fuzz/corpus/whole_czi/zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging.czi), a confocal line scan with a z-stack. Long lines are trimmed.

## What it tells you

- The first lines say what the file is and what it holds.
- The *Experiment* paragraph gathers the settings from the experiment model: technique (with its ontology term), objective, pixel size, z-step, time interval and operator, whichever the file records.
- The geometry paragraph gives the image size, the field of view in µm, the number of focal planes and their spacing, and the bit depth.
- The *Channels* paragraph names each channel with its excitation and emission wavelengths as stored.
- The optics paragraph names the objective with its magnification, numerical aperture and immersion, the instrument and the acquisition software.
- *Caveats* lists what you should not take for granted, here that no acquisition date is stored. Copy only what the file says, and fill the gaps from your lab notes.

The text is generated from header metadata only, with no pixel data read, so it is fast on a multi-gigabyte file. It reports values as the vendor software saved them: "immersion Other" means the file says *other*, not that OpenReadout could not tell.

## Variations

### Ask one question

`--ask` answers in words and names the fields each answer came from, so you can check it:

```text
$ openreadout info zstack.czi --ask "which channels and wavelengths?"
...
Answers:
  [channels] Channel 0 “Ch1” (excitation 561 nm, emission 591 nm); channel 1 “ChS1” (excitation 561 nm, ...
      from: images[0].channels
```

The question's words select a topic: `channels`, `sample`, `method`, `instrument`, `operator`, `acquired`, `technique`, `run_length` and, for mass spectrometry, `polarity` and `ms_levels`. The words that pick each topic are listed in [Metadata](../guides/metadata.md#asking-in-words).

### The numbers, for a table

The same settings are in the JSON, with units and UCUM codes:

```text
$ openreadout info zstack.czi --compact --only /experiment/method/parameters
{..."data":{"/experiment/method/parameters":{..."objective":{"value":"EC Plan-Neofluar 10x/0.30 M27"},
"objective_magnification":{"value":10},...,"pixel_size":{"value":1.6605316419276783,"unit":"µm",
"ucum":"um"},"z_step":{"value":1,"unit":"µm","ucum":"um"}}}}
```

`experiment.provenance` says which field of the file each value was taken from.

### From an assistant

An assistant connected to the [MCP server](../guides/agents.md) calls `openreadout_info` with `view: "explain"`, or with `ask` for one question. It gets back `summary`, `paragraphs`, `caveats`, `suggested_commands` and, for a question, `answers` with the `fields` each came from. Ask it, for example, "Write the acquisition part of the methods for `zstack.czi`, and list what the file does not record." The `summarize_file` prompt asks for the same kind of summary: what the file is, what was measured, how and when.

### Other instruments

The explanation has the same shape for every format, so the same command works for an LC-MS run or a flow tube. These are the *Experiment* paragraphs of two other public test files, `pyteomics-tiny-pwiz.mzML` and `fcsparser-cyflow-cube-8.fcs`:

```text
Experiment: sample “Sample 1” (from `sampleList/sample[0]/@name`); measured by tandem mass spectrometry (CHMO:0000575) (ion source nanoESI, ms levels 1, 2, polarity positive); 5.99 min of data.

Experiment: measured by flow cytometry (CHMO:0000061) (timestep 0.001 s, volume 99,000 nL); 101 s of data; comment “2017_11_02-09_51: Original Data”.
```

## More

- [Metadata conventions and the experiment model](../guides/metadata.md#the-experiment-block): every part of the experiment block.
- [`info` reference](../reference/commands/info.md): all views and flags.
- [MCP tools](../reference/mcp.md#openreadout_info): the `openreadout_info` arguments.
- JSON: [`info --view explain`](../reference/json/info-explain.md).
