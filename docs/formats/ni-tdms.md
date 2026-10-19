# National Instruments TDMS

TDMS is National Instruments' file format for measured data: LabVIEW, DAQmx data logging, SignalExpress, FlexLogger and many custom acquisition programs write it, on electrophysiology rigs, sensor test stands and bench instruments. OpenReadout returns every numeric channel as a trace with its properties, scaled DAQmx raw data included. Written from National Instruments' public description of the format ("TDMS File Format Internal Structure", ni.com), with npTDMS run as a black box to check the values. Provenance: `docs/provenance/ni-tdms.md`.

Format id `ni-tdms`, extension `.tdms`, segment tag `TDSm`. A `.tdms_index` file (tag `TDSh`) holds no values; the reader walks the segments of the `.tdms` file itself and refuses an index file with a hint (exit 6).

## Layout

A file is a series of segments. Each segment is a 28-byte lead-in (tag, table-of-contents flags, version 4712 or 4713, the length of the rest of the segment, the length of its metadata), then metadata, then raw data. The flags say whether the segment has metadata, a new object list, raw data, interleaved or contiguous raw data, big-endian numbers (everything after the flags) and DAQmx raw data. A segment length of all `0xFF` bytes marks a write that was interrupted; its data runs to the end of the file.

**Objects** are the file (`/`), groups (`/'group'`) and channels (`/'group'/'channel'`, quotes doubled inside names). The metadata lists objects with a raw-data index and properties:

- index `0xFFFFFFFF`: no data in this segment; `0`: the same index as in the previous segment; otherwise the index length, then the data type, the array dimension (1), the value count and, for strings, the byte size;
- the bytes `69 12 00 00` (read as the 32-bit number `0x1269`): a DAQmx raw-data index: data type `0xFFFFFFFF`, dimension, value count, the format-changing scalers (DAQmx raw type, raw buffer, byte offset in the buffer row, sample format, scale id) and the widths of the raw buffers. `69 13 00 00` (digital-line scaler) is recognised and not read.
- Properties: name, data type, value. Later segments add objects and properties and overwrite property values; a segment without the new-object-list flag keeps the previous object list and appends new objects to it.

**Raw data.** A segment's raw data is one or more chunks; a chunk holds the values of every channel with data, in object-list order: each channel's values in one block (contiguous), or one value of each channel per row (interleaved). The number of chunks is the raw-data length divided by the chunk size. When a segment ends inside a chunk, the values that are there are read and `notes` say so. String channels store an array of 32-bit end offsets, then the text. DAQmx raw data stores rows of the raw buffers; a channel's value is read at its byte offset in each row with its DAQmx raw type.

## Data types

| code | type | read as |
| --- | --- | --- |
| 1–4, 5–8 | signed and unsigned 8- to 64-bit integers | numbers (`int8` … `uint64`) |
| 9, 10 (and 0x19, 0x1A with units) | 32- and 64-bit floats | numbers (`float32`, `float64`) |
| 0x21 | boolean (1 byte) | 0 or 1 (`bool`) |
| 0x44 | timestamp: 64-bit fraction of 2^-64 s, then signed 64-bit seconds since 1904-01-01 UTC (big-endian files: seconds first) | seconds since 1970-01-01 UTC (`timestamp`) |
| 0x20 | string | listed, not a trace |
| 11, 0x1B, 0x4F, 0x08000C, 0x10000D | extended float, fixed point, complex | not read; `notes` name the channels |
| 0xFFFFFFFF | DAQmx raw data | by the DAQmx raw type |

**DAQmx raw types.** Code 3 is a 16-bit signed integer (inferred from the corpus: two-byte values that, scaled, match npTDMS). Other codes are not read.

## Scaling

Channels whose `NI_Scaling_Status` property is `unscaled` store raw values and describe the scaling in properties: `NI_Number_Of_Scales` = N, and for scale *i* `NI_Scale[i]_Scale_Type` with its parameters and input source. The reader applies scale N−1, after its input source, back to an index with no scale (the raw values). Linear (`NI_Scale[i]_Linear_Slope`, `_Linear_Y_Intercept`, `_Linear_Input_Source`) and polynomial (`NI_Scale[i]_Polynomial_Coefficients_Size`, `_Polynomial_Coefficients[j]`, `_Polynomial_Input_Source`) scales are applied; a channel with another scale type keeps its raw values and `notes` say so. A single linear scale is also the channel's `scale` and `offset`; every applied chain is in `extra.scaling`. (Inferred from the DAQmx files of the corpus and npTDMS's values.)

## Traces

Each group becomes one trace when its numeric channels have the same value count and the same `wf_increment`; otherwise each channel is a trace of its own, named `group/channel`. `sample_rate_hz` is 1 / `wf_increment` (0 when the channel has none), `start_s` the `wf_start_offset`.

| key | meaning |
| --- | --- |
| `traces[].extra.group` | the group name |
| `traces[].extra.wf_increment`, `wf_start_time` | sample interval (s) and start time (ISO 8601 UTC) of the first channel |
| `traces[].extra.group_properties` | the group's properties |
| `traces[0].extra.file_properties` | the file's properties |
| `traces[0].extra.segments`, `tdms_versions` | segment count, TDMS versions in the file |
| `traces[0].extra.layout` | `interleaved`, `big-endian`, `DAQmx raw data` when any segment has them |
| `traces[0].extra.writer` | the writer the file names (`WriterName`, `Writer`, `NI_ExpSoftwareName` file property) |
| `traces[0].extra.string_channels` | string channels: `path`, `values` (count) |
| `channels[].dtype` | the stored type (the DAQmx raw type for DAQmx data) |
| `channels[].unit` | the `unit_string` property; timestamps `s since 1970-01-01T00:00:00Z` |
| `channels[].extra.path`, `value_count`, `storage`, `scaling`, `properties` | object path, values, `DAQmx raw data`, the applied scaling chain, all properties |

`vendor` (`info --view full`) lists every object with its properties, data type and value count, and the first 20 values of string channels (`first_values`). `info --view structure` lists the objects.

## `check`

`check --headers-only`: every lead-in and metadata section (`truncated` when a segment runs past the end of the file or a tag is missing; `channel_not_read` for channels whose values are not read). `check` also reads every value of every numeric channel and every string (`value_count`, `bad_values`).

## Vocabulary

| identifier | meaning |
| --- | --- |
| `TdmsReader` | the TDMS `FormatReader` |
| `TDMS_FORMAT_ID` | `ni-tdms` |
| `TdmsDataset` | an open TDMS file |
| `group`, `wf_increment`, `wf_start_time`, `group_properties`, `file_properties`, `segments`, `tdms_versions`, `layout`, `writer`, `string_channels`, `path`, `value_count`, `storage`, `scaling`, `properties`, `objects`, `data_type`, `first_values` | JSON keys (tables above) |
| `truncated`, `channel_not_read`, `value_count`, `bad_values` | `check` finding codes |
