# planes

`planes` reads every plane of a file, or a selection, and prints each plane's dimensions and xxh3-128 hash. It reads all pixel data. It is a command-line tool for scripts: to check that a copy holds the same data, [`compare`](compare.md) does the hashing for you.

```text
openreadout planes [OPTIONS] <FILE>...
```

## Flags

- `--json`: print the JSON wrapper instead of text.
- `--image N`: only this image.
- `--select SEL`: plane selection such as `c=0`, `z=2-5`, `t=0,3`. Repeatable.
- `--level N`: pyramid level, 0 = full resolution.
- `--region X,Y,W,H`: only this rectangle of each plane, in the pixels of `--level`.
- `--dump-dir DIR`: also write each plane's raw little-endian samples to `DIR/image<i>_c<c>_z<z>_t<t>.bin`.

`planes` takes the flags in [Several inputs](index.md#several-inputs).

## Example

```bash
openreadout planes stack.nd2 --select c=0 --json
```

## JSON

[`planes`](../json/planes.md).

Run `openreadout planes --help` for the full help of your installed version.
