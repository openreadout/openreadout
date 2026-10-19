# Provenance log — FIF (`.fif`, MEGIN / Elekta Neuromag and MNE)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- MNE-Python 1.13.2 (BSD-3-Clause, https://github.com/mne-tools/mne-python, the copy installed in
  `oracle/.venv`): `mne/_fiff/tag.py` (the 16-byte big-endian tag header, the data types, the
  96-byte channel-info struct, the id struct, directory entries), `mne/_fiff/open.py` (file id
  and directory pointer, the sequential scan when there is no directory, split-file references),
  `mne/_fiff/tree.py` (blocks from block-start and block-end tags), `mne/_fiff/constants.py`
  (tag, block, channel-kind and unit codes), `mne/_fiff/meas_info.py` (the measurement info,
  extended channel blocks, calibration = range × cal), `mne/io/fiff/raw.py` (data buffers, first
  sample, data skips, buffer types) and `mne/annotations.py` (the annotation block).
- MNE-Python is also run as the oracle (`oracle/gen.py`, `fif()`), as a black box.

**Corpus files used:** `mnepython-test-raw-fif`, `mnepython-test-chpi-raw-sss-fif`,
`mnepython-test-ctf-comp-raw-fif`, `mnepython-test-withbads-raw-fif`, `mnepython-kit-test-bin-raw-fif`,
`mnepython-bti-exported4d-linux-raw-fif`, `mnepython-brainvision-test-bin-raw-fif`,
`zenodo-14564285-resting-eo-200hz-raw`, `zenodo-14564285-sub-02-mne-raw`,
`zenodo-18186380-prob1-stimuli-channel-raw`, `zenodo-22820861-sub-0006-sr200-raw`,
`openneuro-ds000248-sub-emptyroom-task-noise`.

**Inferred from the files:** `mnepython-kit-test-bin-raw-fif` (written by MNE-C) has a directory
that ends with an entry of −1s, and tags whose `next` points back to an earlier position (the
environment block is written last and linked in): the scan follows `next` wherever it points and
stops only at a position it has seen. The channel-kind names, unit symbols and the trace layout
(one trace, gaps as sweeps) are our own.
