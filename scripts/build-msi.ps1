# Builds booto-writer.exe (static CRT, no VC++ runtime needed) and packs it into an MSI with WiX 5.
# Usage: scripts/build-msi.ps1 <output.msi>   Writes <output.msi> and <output.msi>.sha256.
param([Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force (Split-Path $Out -Parent) | Out-Null

$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --release --locked
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version

dotnet tool update --global wix --version 5.0.2
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$env:PATH += ";$env:USERPROFILE\.dotnet\tools"
wix build wix/main.wxs -arch x64 -d "Version=$version" -d "Exe=target\release\booto-writer.exe" -o $Out
if ($LASTEXITCODE) { exit $LASTEXITCODE }
# WiX writes a .wixpdb debug file next to the MSI; it is not a download.
Remove-Item ([IO.Path]::ChangeExtension($Out, 'wixpdb')) -ErrorAction SilentlyContinue

$name = Split-Path $Out -Leaf
$hash = (Get-FileHash $Out -Algorithm SHA256).Hash.ToLower()
Set-Content -Path "$Out.sha256" -Value "$hash  $name" -Encoding ascii
Write-Host "Built $Out ($version, SHA-256 $hash)"
