# Provenance: Arbin `.res` result files (and the Microsoft Jet database container)

## 2026-09-26: initial reader (`arbin-res`; Richard Zimring with Claude as assistant)

**Corpus files used (development):**

- GitHub jepegit/cellpy (MIT, commit 931e0581): `testdata/data/20160805_test001_45_cc_01.res`
  (`echem-cellpy-arbin-test001`), `aux_multi_x.res` (`echem-cellpy-arbin-aux-multi`),
  `aux_one_x_dx.res` (`echem-cellpy-arbin-aux-dxdt`) and `examples/data/20210210_FC_01_cc_01.res`
  (`echem-cellpy-arbin-fc01`): Arbin MITS Pro 4.32 and 7.00 result databases, with auxiliary
  channels in the `aux_*` files; and cellpy's golden output for the first
  (`tests/data/goldens/loader_arbin_res/raw.parquet`).
- GitHub be-smith/navani (MIT, commit d9892a3): `Example_data/arbin_example.res`
  (`echem-navani-arbin`).
- GitHub ahpohl/convpot (MIT, commit f626cdf): `test/arbintest.res` (`echem-convpot-arbin`).
- Zenodo 21631502 (Clark, Tolchard, Kopljar; SINTEF battery test datasets, CC-BY-4.0):
  `Abbta__PW-Na-ECDEC-01-034__20231222__Cycling__RT__Arbin.res`
  (`echem-zenodo21631502-sintef-arbin`).

Five depositors: one (a GitHub repository, MIT) is held out (draw 2026-09-26) and was not
opened.

**Prior art consulted:**

- access_parser (Claroty, Apache-2.0, https://github.com/claroty/access_parser, v0.0.6 / commit
  7b733913), read as documentation of the Jet 4 page, table-definition and record layout
  (`parsing_primitives.py`, `access_parser.py`, `utils.py`), and run as a black box to produce the
  ground truth (`oracle/arbin_oracle.py`).
- Jackcess (Health Market Science / James Ahlborn, Apache-2.0,
  https://github.com/jahlborn/jackcess, master 2026-09-13), read as documentation of three details
  access_parser does not handle: compressed text (`ColumnImpl.decodeTextValue`: after `FF FE`, a
  zero byte switches between one byte per character and UTF-16LE, starting with one byte), long
  values (`LongValueColumnImpl.readLongValue`: the two flag bits 0x80 inline, 0x40 one other row,
  0 a chain whose rows begin with the pointer to the next), and row bounds and overflow rows
  (`TableImpl.positionAtRowData`, `findRowStart`/`findRowEnd`: offsets masked with 0x1FFF, a row
  ends at the previous row's start, the overflow pointer is a row byte and a 3-byte page; the
  variable offsets start 4 bytes before the null mask).
- The column names and units of Arbin's tables as cellpy (MIT) documents them
  (`src/cellpy/readers/instruments/arbin_res.py` and its golden `raw_units.json`): time in s,
  current in A, voltage in V, capacity in A·h, energy in W·h.

**What we inferred / implemented (from the files and the documentation above):**

- A `.res` file is a Jet 4 database (`00 01 00 00` `Standard Jet DB`, version 1 at 0x14,
  4096-byte pages). The header page's masked region (database password and key) is not read; a
  database whose catalog page is not a table definition is refused as encrypted.
- Catalog: table `MSysObjects` (table definition on page 2); user tables are rows of `Type` 1 whose
  `Flags` do not mark a system table; `Id` is their table-definition page.
- Table definitions, data pages and records as in `docs/formats/arbin-res.md`. The null mask is
  indexed by the column number and sized by the row's own column count (a column added after the
  row was written is null).
- Rows come from every data page whose owner is the table (access_parser's approach; the usage
  maps are not read). The row count read is checked against the definition's count (a finding when
  they differ; they agree on every file).
- Arbin's rows are not stored in data-point order (`echem-cellpy-arbin-fc01`,
  `echem-navani-arbin`, `echem-convpot-arbin`): the reader orders them by `Test_ID` and
  `Data_Point`, as cellpy's golden output does.
- Auxiliary inputs: `Data_Type` 0 is a voltage and 1 a temperature (the unit
  `Aux_Global_Data_Table` records for them is `V` and `C` in all four files with auxiliary data);
  one value per data point and input.
- `Global_Table` `MASS` is in g and `Specific_Capacity` in A·h/g: inferred from the magnitudes
  (0.00085 and 3.579 for a silicon anode; 0.0226 and 0.16 for a full cell).
- `Start_DateTime` and `DateTime` are OLE Automation dates in the tester's local time (cellpy's
  golden gives the same wall-clock times with no zone).
