# winget manifests (not yet submitted)

The three YAML files here are **templates** for the [Windows Package Manager](https://learn.microsoft.com/windows/package-manager/) community repository ([microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)). They describe the release zip `openreadout-x86_64-pc-windows-msvc.zip` as a *portable* package: winget unpacks it and puts `openreadout` on the `PATH` through a command alias. Nothing has been submitted; the package identifier `OpenReadout.OpenReadout` is unclaimed until the first pull request to winget-pkgs is merged.

| file | manifest type |
| --- | --- |
| `OpenReadout.OpenReadout.yaml` | version |
| `OpenReadout.OpenReadout.installer.yaml` | installer (URL + SHA-256 of the zip) |
| `OpenReadout.OpenReadout.locale.en-US.yaml` | default locale (description, license, tags) |

## Render the manifests for a release

After a release is published (its `SHA256SUMS` asset is needed):

```bash
gh release download v0.1.0 -p SHA256SUMS -D /tmp/ic
cargo xtask winget-manifest --version 0.1.0 --sums /tmp/ic/SHA256SUMS --out target/winget
# -> target/winget/manifests/o/OpenReadout/OpenReadout/0.1.0/*.yaml
```

## Validate and test locally (on Windows)

```powershell
winget validate --manifest target\winget\manifests\o\OpenReadout\OpenReadout\0.1.0
winget settings --enable LocalManifestFiles     # once, as administrator
winget install --manifest target\winget\manifests\o\OpenReadout\OpenReadout\0.1.0
openreadout --version
winget uninstall OpenReadout.OpenReadout
```

## Submit (maintainers, once the repository is public)

Prerequisites: the GitHub release must be public (winget's validation pipeline downloads the zip and scans it), and the binary should ideally be Authenticode-signed; unsigned portable packages are accepted but may be flagged by SmartScreen and the automated malware scan can delay review.

1. Fork `microsoft/winget-pkgs`, copy the rendered `manifests/o/OpenReadout/OpenReadout/<version>/` directory into the fork at the same path, commit and open a pull request. Or let Microsoft's tool do it:
   ```powershell
   winget install wingetcreate
   wingetcreate submit --token <GITHUB_PAT> target\winget\manifests\o\OpenReadout\OpenReadout\0.1.0
   ```
2. Sign the Microsoft CLA when the bot asks (first submission only), then wait for the validation pipeline and a moderator.
3. For later releases: `wingetcreate update OpenReadout.OpenReadout --version <new> --urls https://github.com/openreadout/openreadout/releases/download/v<new>/openreadout-x86_64-pc-windows-msvc.zip --submit --token <PAT>` (or render these templates again). This can be automated with a release job using a fine-grained PAT stored as a secret; it is deliberately not wired up yet.

Windows on Arm runs the x64 binary under emulation; a native `aarch64-pc-windows-msvc` build would add a second `Installers` entry.
