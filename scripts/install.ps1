# OpenReadout installer for Windows. Usage: irm https://raw.githubusercontent.com/openreadout/openreadout/main/scripts/install.ps1 | iex
$ErrorActionPreference = "Stop"
$repo = "openreadout/openreadout"
$dir = if ($env:OPENREADOUT_INSTALL_DIR) { $env:OPENREADOUT_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\openreadout" }
$url = "https://github.com/$repo/releases/latest/download/openreadout-x86_64-pc-windows-msvc.zip"
$tmp = Join-Path $env:TEMP "openreadout-$([guid]::NewGuid()).zip"
Write-Host "downloading $url"
Invoke-WebRequest -Uri $url -OutFile $tmp
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Expand-Archive -Path $tmp -DestinationPath $dir -Force
Remove-Item $tmp
Write-Host "installed $dir\openreadout.exe"
if (-not (($env:Path -split ";") -contains $dir)) {
  [Environment]::SetEnvironmentVariable("Path", [Environment]::GetEnvironmentVariable("Path", "User") + ";$dir", "User")
  Write-Host "added $dir to your user PATH (open a new terminal)"
}
Write-Host "agent skill: openreadout skill --install all    |  MCP: openreadout mcp --config claude"
