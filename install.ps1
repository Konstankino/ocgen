<#
  ocgen Windows installer / updater.

  Install or update to the latest release:
    irm https://raw.githubusercontent.com/Konstankino/ocgen/main/install.ps1 | iex

  Or a specific version:
    & ([scriptblock]::Create((irm https://raw.githubusercontent.com/Konstankino/ocgen/main/install.ps1))) -Version v0.2.0

  Re-run any time to update — it always fetches the requested (default: latest) release.
#>
[CmdletBinding()]
param(
  # GitHub owner/repo that hosts the releases.
  [string]$Repo = 'Konstankino/ocgen',
  # Release tag to install; 'latest' resolves the newest release.
  [string]$Version = 'latest',
  # Where to install ocgen.exe.
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'ocgen\bin')
)

$ErrorActionPreference = 'Stop'
$target = 'x86_64-pc-windows-msvc'

# Resolve the release tag.
$headers = @{ 'User-Agent' = 'ocgen-installer'; 'Accept' = 'application/vnd.github+json' }
if ($Version -eq 'latest') {
  $tag = (Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" -Headers $headers).tag_name
} else {
  $tag = $Version
}
if (-not $tag) { throw "Could not resolve a release for $Repo." }

$asset = "ocgen-$tag-$target.zip"
$url = "https://github.com/$Repo/releases/download/$tag/$asset"
Write-Host "Downloading $asset ..."

$tmpZip = New-TemporaryFile
Invoke-WebRequest $url -OutFile $tmpZip -Headers $headers -UseBasicParsing
$extract = Join-Path $env:TEMP "ocgen-$tag-extract"
Remove-Item $extract -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive $tmpZip $extract -Force
$exe = Get-ChildItem $extract -Recurse -Filter 'ocgen.exe' | Select-Object -First 1
if (-not $exe) { throw "ocgen.exe not found in $asset." }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item $exe.FullName (Join-Path $InstallDir 'ocgen.exe') -Force
Remove-Item $tmpZip, $extract -Recurse -Force -ErrorAction SilentlyContinue

# Add the install dir to the user PATH (persistent) if it is not already there.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$InstallDir*") {
  [Environment]::SetEnvironmentVariable('Path', "$userPath;$InstallDir", 'User')
  $env:Path += ";$InstallDir"
  Write-Host "Added $InstallDir to your PATH — open a new terminal to pick it up."
}

Write-Host "Installed ocgen $tag to $InstallDir"
& (Join-Path $InstallDir 'ocgen.exe') --version
