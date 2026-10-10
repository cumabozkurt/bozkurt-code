# SPDX-License-Identifier: MIT OR Apache-2.0
<#
.SYNOPSIS
  BOZKURT CODE installer for Windows.

.DESCRIPTION
  Downloads bz.exe, bozkurtd.exe and bozkurt-mcp.exe from a GitHub release,
  verifies the SHA-256 against SHA256SUMS and installs them into
  $env:BOZKURT_HOME\bin (default %LOCALAPPDATA%\Bozkurt\bin). No admin rights needed.

  irm https://raw.githubusercontent.com/cumabozkurt/bozkurt-code/main/scripts/install.ps1 | iex

.PARAMETER Version
  Release tag, e.g. v1.0.0. Default: latest (or $env:BOZKURT_VERSION).
.PARAMETER InstallHome
  Install root. Default: $env:BOZKURT_HOME or %LOCALAPPDATA%\Bozkurt.
.PARAMETER Repo
  GitHub repository owner/name. Default: $env:BOZKURT_REPO or cumabozkurt/bozkurt-code.
.PARAMETER AddToPath
  Also add the bin directory to the *user* PATH (HKCU). Off by default.
#>
[CmdletBinding()]
param(
  [string]$Version = $(if ($env:BOZKURT_VERSION) { $env:BOZKURT_VERSION } else { 'latest' }),
  [string]$InstallHome = $(if ($env:BOZKURT_HOME) { $env:BOZKURT_HOME } else { Join-Path $env:LOCALAPPDATA 'Bozkurt' }),
  [string]$Repo = $(if ($env:BOZKURT_REPO) { $env:BOZKURT_REPO } else { 'cumabozkurt/bozkurt-code' }),
  [switch]$AddToPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

function Say([string]$msg) { Write-Host "bozkurt: $msg" }
function Fail([string]$msg) { throw "bozkurt: error: $msg" }

$arch = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
switch ($arch) {
  'AMD64' { $target = 'x86_64-pc-windows-msvc' }
  'ARM64' {
    Say 'no native ARM64 build yet; installing the x64 build (runs under Windows emulation)'
    $target = 'x86_64-pc-windows-msvc'
  }
  default { Fail "unsupported architecture: $arch" }
}

$asset = "bozkurt-cli-$target.zip"
if ($Version -eq 'latest') {
  $base = "https://github.com/$Repo/releases/latest/download"
} else {
  if (-not $Version.StartsWith('v')) { $Version = "v$Version" }
  $base = "https://github.com/$Repo/releases/download/$Version"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("bozkurt-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Say "installing BOZKURT CODE ($Version) for $target"
  $zip = Join-Path $tmp $asset
  $sums = Join-Path $tmp 'SHA256SUMS'
  Say "downloading $base/$asset"
  Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip
  Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums

  $expected = $null
  foreach ($line in Get-Content $sums) {
    $parts = $line -split '\s+', 2
    if ($parts.Count -eq 2 -and $parts[1].TrimStart('*').Trim() -eq $asset) { $expected = $parts[0].ToLowerInvariant() }
  }
  if (-not $expected) { Fail "$asset is not listed in SHA256SUMS" }
  $actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLowerInvariant()
  if ($actual -ne $expected) { Fail "checksum mismatch for $asset (expected $expected, got $actual)" }
  Say "checksum OK ($actual)"

  $extract = Join-Path $tmp 'x'
  Expand-Archive -Path $zip -DestinationPath $extract -Force

  $binDir = Join-Path $InstallHome 'bin'
  New-Item -ItemType Directory -Force -Path $binDir | Out-Null
  foreach ($b in 'bz.exe', 'bozkurtd.exe', 'bozkurt-mcp.exe') {
    $src = Get-ChildItem -Path $extract -Recurse -File -Filter $b | Select-Object -First 1
    if (-not $src) { Fail "archive does not contain $b" }
    $dest = Join-Path $binDir $b
    if (Test-Path $dest) {
      # A running exe cannot be overwritten but can be renamed out of the way.
      $old = "$dest.old"
      Remove-Item $old -Force -ErrorAction SilentlyContinue
      Move-Item $dest $old -Force
    }
    Copy-Item $src.FullName $dest -Force
  }
  Say "installed bz.exe, bozkurtd.exe, bozkurt-mcp.exe -> $binDir"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $onPath = ($userPath -split ';') -contains $binDir
  if (-not $onPath) {
    if ($AddToPath) {
      $newPath = if ([string]::IsNullOrEmpty($userPath)) { $binDir } else { "$userPath;$binDir" }
      [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
      $env:Path = "$env:Path;$binDir"
      Say "added $binDir to your user PATH (open a new terminal to pick it up)"
    } else {
      Say ''
      Say "$binDir is not on your PATH. Either re-run with -AddToPath, or run:"
      Say ''
      Say "    [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path','User'));$binDir`", 'User')"
      Say ''
    }
  }
  Say 'next steps:  bz doctor   then   bz dns setup; bz trust   then   cd your-project; bz link'
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
