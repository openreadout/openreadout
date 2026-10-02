# self

`self` reports on and sets up this installation: supported formats, a self-test, JSON Schemas, the agent skill, shell completions and man pages.

```text
openreadout self <SUBCOMMAND> [OPTIONS]
```

## Subcommands

### formats

```text
openreadout self formats [--json]
```

Lists every supported format with its id, read and write support, confidence and known gaps. The same information, per format, is in [Formats](../../formats/index.md).

```console
$ openreadout self formats
id     name              read  write  confidence  extensions
czi    Zeiss CZI         yes   no     medium      czi
         gap: JPEG (id 1) subblocks decode with jpeg-decoder, …
…
```

### doctor

```text
openreadout self doctor [--write-fixtures DIR] [--json]
```

Checks this installation: version, build, compiled features, formats and the MCP server. It then runs a self-test on two small synthetic files it generates (a TIFF and an FCS file), through the readers and the analyses. It needs no data of your own. Exit 0 when every check passes, 1 otherwise.

- `--write-fixtures DIR`: write the two self-test files (`doctor.tif`, `doctor.fcs`) into DIR and exit. Use them to try commands or to attach to a bug report.

### schema

```text
openreadout self schema <OF>
```

Prints the JSON Schema of a command's `data` payload. `OF` is one of the names in the [JSON reference](../json/index.md), such as `info`, `info-full`, `check-against`, `stats-wells`, `batch-table`, `search-health` or `envelope`.

### skill

```text
openreadout self skill [--install claude|agents|all]
```

Prints the agent skill (`SKILL.md`), or installs it.

- `--install claude`: into `~/.claude/skills`.
- `--install agents`: into `~/.agents/skills`, read by Codex, Cursor, Copilot and Gemini.
- `--install all`: both.

See [AI agents](../../guides/agents.md).

### completions

```text
openreadout self completions <bash|zsh|fish|powershell|elvish>
```

Prints a shell completion script to stdout.

```bash
openreadout self completions bash > ~/.local/share/bash-completion/completions/openreadout
openreadout self completions zsh  > "${fpath[1]}/_openreadout"
openreadout self completions fish > ~/.config/fish/completions/openreadout.fish
openreadout self completions powershell >> $PROFILE
```

### man

```text
openreadout self man --out <DIR>
```

Writes man pages (`openreadout.1` and one per command) into DIR, which is created if missing.

```bash
openreadout self man --out ~/.local/share/man/man1
```

## JSON

[`self formats`](../json/formats.md), [`self doctor`](../json/doctor.md).

Run `openreadout self <subcommand> --help` for the full help of your installed version.
