# OpenReadout installer for Windows. Usage: irm https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.ps1 | iex
# Installs into %LOCALAPPDATA%\Programs\openreadout (or $env:OPENREADOUT_INSTALL_DIR) and adds it to
# your user PATH. $env:OPENREADOUT_VERSION = "v0.1.0" installs that release instead of the latest
# one. The archive is checked against the release's SHA256SUMS before anything is installed.
# Windows on Arm runs the x64 build under emulation.
& {
  $ErrorActionPreference = "Stop"
  # Windows PowerShell 5.1 draws a progress bar that slows downloads a lot, and older .NET
  # versions do not offer TLS 1.2, which GitHub requires, unless asked.
  $ProgressPreference = "SilentlyContinue"
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

  $repo = "openreadout/openreadout"
  $dir = if ($env:OPENREADOUT_INSTALL_DIR) { $env:OPENREADOUT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\openreadout" }
  $version = if ($env:OPENREADOUT_VERSION) { $env:OPENREADOUT_VERSION } else { "latest" }
  if ($version -ne "latest" -and -not $version.StartsWith("v")) { $version = "v$version" }
  $base = if ($version -eq "latest") { "https://github.com/$repo/releases/latest/download" } else { "https://github.com/$repo/releases/download/$version" }
  $asset = "openreadout-x86_64-pc-windows-msvc.zip"

  $tmp = Join-Path ([IO.Path]::GetTempPath()) "openreadout-$([guid]::NewGuid())"
  New-Item -ItemType Directory -Force -Path $tmp | Out-Null
  try {
    Write-Host "downloading $base/$asset"
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $tmp "SHA256SUMS")
    $expected = $null
    foreach ($line in Get-Content (Join-Path $tmp "SHA256SUMS")) {
      $parts = $line -split '\s+', 2
      if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $asset) { $expected = $parts[0].ToLowerInvariant() }
    }
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLowerInvariant()
    if (-not $expected -or $actual -ne $expected) {
      throw "checksum mismatch for $asset (expected $expected, got $actual); nothing was installed"
    }
    Write-Host "sha256 ok"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $dir -Force
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }

  $exe = Join-Path $dir "openreadout.exe"
  Write-Host "installed $exe ($(& $exe --version))"
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if (-not $userPath) { $userPath = "" }
  if (-not (($userPath -split ";") -contains $dir)) {
    [Environment]::SetEnvironmentVariable("Path", (($userPath.TrimEnd(";"), $dir) -join ";").TrimStart(";"), "User")
    Write-Host "added $dir to your user PATH"
  }
  # This terminal too, so `openreadout` works right away.
  if (-not (($env:Path -split ";") -contains $dir)) { $env:Path = "$env:Path;$dir" }
  Write-Host "agent skill: openreadout self skill --install all    |  MCP: openreadout mcp --install claude-desktop (or claude, cursor, codex, vscode, gemini)"
}
