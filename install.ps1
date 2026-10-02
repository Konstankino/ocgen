<#
  ocgen Windows installer / updater.

  Install or update to the latest release:
    irm https://raw.githubusercontent.com/Konstankino/ocgen/main/install.ps1 | iex

  Or a specific version:
    & ([scriptblock]::Create((irm https://raw.githubusercontent.com/Konstankino/ocgen/main/install.ps1))) -Version v0.2.0

  Re-run any time to update - it always fetches the requested (default: latest) release.

  The archive is checked against the release's SHA256SUMS before anything is
  installed; a release from before checksums were published installs with a
  warning. -SkipVerify (or OCGEN_INSTALL_SKIP_VERIFY=1) installs without the check.

  Updating works while ocgen runs (the notes viewer, a hook): the running
  ocgen.exe is renamed to ocgen.exe.old, which a later run deletes.
#>
[CmdletBinding()]
param(
  # GitHub owner/repo that hosts the releases.
  [string]$Repo = 'Konstankino/ocgen',
  # Release tag to install; 'latest' resolves the newest release.
  [string]$Version = 'latest',
  # Where to install ocgen.exe.
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'ocgen\bin'),
  # Install without checking the archive against the release's SHA256SUMS.
  [switch]$SkipVerify
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
$base = "https://github.com/$Repo/releases/download/$tag"
Write-Host "Downloading $asset ..."

# Expand-Archive (Windows PowerShell 5.1) only accepts a path ending in .zip, so
# name the download explicitly instead of using a .tmp temporary file.
$tmpZip = Join-Path $env:TEMP ("ocgen-$tag-" + [guid]::NewGuid().ToString('N') + '.zip')
$extract = Join-Path $env:TEMP ("ocgen-$tag-extract-" + [guid]::NewGuid().ToString('N'))
$tmpSums = "$tmpZip.SHA256SUMS"
try {
  Invoke-WebRequest "$base/$asset" -OutFile $tmpZip -Headers $headers -UseBasicParsing

  # Check the archive against the release's SHA256SUMS ("<sha256>  <file>" lines).
  $noSums = $false
  if ($SkipVerify -or $env:OCGEN_INSTALL_SKIP_VERIFY -eq '1') {
    Write-Host 'Skipping the checksum check.'
  } else {
    try {
      Invoke-WebRequest "$base/SHA256SUMS" -OutFile $tmpSums -Headers $headers -UseBasicParsing
    } catch {
      $status = $_.Exception.Response.StatusCode
      if ($status -and [int]$status -eq 404) {
        # Releases before v0.4.7 shipped without checksums.
        $noSums = $true
      } else {
        throw "Cannot verify ${asset}: could not download the release's SHA256SUMS ($base/SHA256SUMS: $($_.Exception.Message)). Try again, or re-run with -SkipVerify (or OCGEN_INSTALL_SKIP_VERIFY=1) to install it without the check."
      }
    }
  }
  if ($noSums) {
    Write-Warning "This release has no SHA256SUMS (it predates checksums) - installing $asset unchecked."
  } elseif (-not ($SkipVerify -or $env:OCGEN_INSTALL_SKIP_VERIFY -eq '1')) {
    $want = $null
    foreach ($line in Get-Content -LiteralPath $tmpSums) {
      $fields = $line.Trim() -split '\s+', 2
      if ($fields.Count -eq 2 -and $fields[1].TrimStart('*') -eq $asset) {
        $want = $fields[0].ToLowerInvariant()
        break
      }
    }
    if (-not $want) { throw "Cannot verify ${asset}: SHA256SUMS does not list it." }
    $got = (Get-FileHash -LiteralPath $tmpZip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($got -ne $want) {
      throw "Checksum mismatch for $asset - not installing.`n  expected $want`n  got      $got"
    }
    Write-Host "Checksum OK ($asset)."
  }

  Expand-Archive $tmpZip $extract -Force
  $exe = Get-ChildItem $extract -Recurse -Filter 'ocgen.exe' | Select-Object -First 1
  if (-not $exe) { throw "ocgen.exe not found in $asset." }

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  $dest = Join-Path $InstallDir 'ocgen.exe'

  # Old binaries renamed aside by earlier updates: delete the ones no longer running.
  Get-ChildItem -LiteralPath $InstallDir -Filter 'ocgen*.exe.old' -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

  # Copy next to the target first, so the switch below is two renames.
  $new = Join-Path $InstallDir ('.ocgen.' + [guid]::NewGuid().ToString('N') + '.exe.new')
  Copy-Item -LiteralPath $exe.FullName -Destination $new -Force
  try {
    # A running exe can't be overwritten or deleted, but it can be renamed:
    # move it aside, then put the new one in its place.
    $aside = $null
    if (Test-Path -LiteralPath $dest) {
      $aside = "$dest.old"
      if (Test-Path -LiteralPath $aside) {
        # Still running from an earlier update: pick a fresh name.
        $aside = Join-Path $InstallDir ('ocgen.' + [guid]::NewGuid().ToString('N') + '.exe.old')
      }
      Move-Item -LiteralPath $dest -Destination $aside
    }
    try {
      Move-Item -LiteralPath $new -Destination $dest
    } catch {
      if ($aside) { Move-Item -LiteralPath $aside -Destination $dest -ErrorAction SilentlyContinue }
      throw
    }
    if ($aside) { Remove-Item -LiteralPath $aside -Force -ErrorAction SilentlyContinue }
  } finally {
    Remove-Item -LiteralPath $new -Force -ErrorAction SilentlyContinue
  }
} finally {
  Remove-Item $tmpZip, $tmpSums, $extract -Recurse -Force -ErrorAction SilentlyContinue
}

# Add the install dir to the user PATH (persistent) if it is not already there.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$InstallDir*") {
  [Environment]::SetEnvironmentVariable('Path', "$userPath;$InstallDir", 'User')
  $env:Path += ";$InstallDir"
  Write-Host "Added $InstallDir to your PATH - open a new terminal to pick it up."
}

Write-Host "Installed ocgen $tag to $InstallDir"
& (Join-Path $InstallDir 'ocgen.exe') --version
