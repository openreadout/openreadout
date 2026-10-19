# HORIBA LabSpec 6 provenance

## 2026-10-19 — the reader (Richard Zimring with Claude as assistant)

**Written before the reader.** Format `horiba-labspec`.

**Corpus files used** (all in `corpus/manifest.toml`, each with LabSpec's text export of the same file as `oracle-export`): `dvn-ulj5h2-glass`, `dvn-ulj5h2-cyclo-3sec`, `dvn-ulj5h2-a1ker0-6spc`, `dvn-ulj5h2-blank-10spc` (Harvard Dataverse doi:10.7910/DVN/ULJ5H2, CC0); `borealis-ho6ler-tosoh` (Borealis doi:10.5683/SP3/HO6LER, CC BY 4.0); `figshare25126925-nit-01`, `figshare25126925-beads-02` (figshare 25126925, CC BY 4.0); `figshare31861468-sampe-1-solid` (figshare 31861468, CC BY 4.0); `ncl22790156-calcium-carbonate` (figshare 22790156, CC BY 4.0); `horiba-raman-aln-gd2o3-map` (github.com/ccoverstreet/horiba-raman, MIT). The encoded file of Zenodo 21324687 (CC BY 4.0) was looked at only to see that its body is not stored plainly. No file is held out yet.

**Prior art consulted:** none.

**Inferred from the files** (hex dumps, and differential comparison with each file's export):

- All files start `LabSpec6`. The u32 at byte 12 is 0 (Dataverse, Borealis, figshare 25126925 and 22790156) or 8 (figshare 31861468, the map). In the Zenodo 21324687 file it is 1 and the bytes from offset 0x40 have an entropy of 7.97 bits per byte, so its content is not stored plainly.
- Searching each file for the export's first intensity as a float32 finds a run of the export's values, and the 24 bytes before the run end in the count of values. Those 24-byte runs repeat through the file with a u32 type (2 to 9, 0x1006 to 0x1009) first, a 4-byte key at byte 8 and an 8-byte value at byte 16. The record before each run of intensities (or a few records before it) has type 0x1006 and key `db d4 6c 7d`, and its last u32 is the number of values. Between that record and the values there are zero to four more records (keys `typm`, `uni|`, `temx`, `tam`). In every file the values start where the next record does not.
- Type-7 records with key `typm` hold `Intens` and `Spectr`, and with key `uni|` hold `Cnt` and `1/cm` (`nm` in the Dataverse glass file, whose export x runs from 239.3 to 518.2). The `Spectr` array equals the export's x column and the `Intens` array its y column, to the export's six significant digits.
- The map has an `Intens` array of 35,856 values, a `Spectr` array of 498, an `X` array of 12 and a `Y` array of 6 (both `µm`). The export lists 72 rows: row k has Y = `Y[k / 12]`, X = `X[k mod 12]` and the values `Intens[498 k ...]`.
- `figshare25126925-beads-02` has a second `Intens`/`Spectr` pair after the first; the export holds only the first.
