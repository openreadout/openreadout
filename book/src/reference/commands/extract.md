# extract

`extract` writes one attachment that a file embeds to a new file, as it is stored. A CZI can carry a `Thumbnail` JPEG, `Label` and `SlidePreview` images, and `TimeStamps`. `info --view structure` lists the attachments with the exact `extract` command.

```text
openreadout extract [OPTIONS] <FILE> <ATTACHMENT>
```

MCP: `openreadout_extract`.

## Flags

- `ATTACHMENT`: the attachment's name as `info --view structure` shows it, or `#<index>`.
- `-o`, `--output FILE`: where to write it. Default: `<input stem>.<name>.<ext>` next to the input.
- `--overwrite`: replace an existing output file.
- `--json`: print the JSON wrapper instead of text.

The written file is read back and compared before it gets its final name.

## Example

```bash
openreadout extract slide.czi Label
```

## JSON

[`extract`](../json/extract.md).

Run `openreadout extract --help` for the full help of your installed version.
