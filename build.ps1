#Requires -Version 5.1
<#
  ShotGlance - single build entry point (Git area).
  ---------------------------------------------------------------------------
  This script is the ONLY place that writes into the "build area" (build\).
  It never stores configuration there: npm / cargo / vite / tauri configs all
  stay next to the source they belong to, because those tools look them up at
  fixed relative paths.

  Zones:
    Git area   : .gitignore .gitattributes README.md LICENSE CHANGELOG.md
                 build.ps1 verify-env.ps1 + shotglance\  (all committed)
    build area : build\dist    vite frontend bundle
                 build\target  cargo target (release\bundle\nsis\*.exe)
                 build\release archived installers (survives a target wipe)
                 build\logs    build logs

  Usage:
    .\build.ps1                 full build  (frontend + tauri + archive)
    .\build.ps1 -Target Frontend    only tsc + vite  -> build\dist
    .\build.ps1 -Target Backend     only tauri build -> build\target
    .\build.ps1 -DebugBuild         debug profile instead of release
    .\build.ps1 -Clean              wipe build\dist and build\target first
  ---------------------------------------------------------------------------
  NOTE: keep this file pure ASCII. PowerShell 5.1 reads BOM-less UTF-8 as ANSI
  and would corrupt non-ASCII characters.
#>
[CmdletBinding()]
param(
    [ValidateSet('All', 'Frontend', 'Backend')]
    [string]$Target = 'All',
    [switch]$DebugBuild,
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'

$root    = $PSScriptRoot
$proj    = Join-Path $root 'shotglance'
$tauri   = Join-Path $proj 'src-tauri'
$build   = Join-Path $root 'build'
$distDir = Join-Path $build 'dist'
$targetDir = Join-Path $build 'target'
$relDir  = Join-Path $build 'release'
$logDir  = Join-Path $build 'logs'

foreach ($d in @($build, $distDir, $targetDir, $relDir, $logDir)) {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# ---- transcript (audit trail in build\logs) --------------------------------
$logPath = Join-Path $logDir ('build-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
try { Start-Transcript -Path $logPath -Force | Out-Null } catch { $logPath = $null }

# ---- toolchain -------------------------------------------------------------
# Node: prefer PATH, else pick the highest version under the managed root. The
# folder name carries a build suffix (22.22.2-2, 22.22.2-6, ...) that changes
# whenever the runtime is reinstalled, so never hardcode it.
$managedNodeRoot = 'C:\Users\Administrator\.workbuddy\binaries\node\versions'
if (-not (Get-Command node -ErrorAction SilentlyContinue) -and (Test-Path $managedNodeRoot)) {
    $managedNode = Get-ChildItem $managedNodeRoot -Directory |
        Where-Object { Test-Path (Join-Path $_.FullName 'node.exe') } |
        Sort-Object { [version]($_.Name -replace '-.*$', '') } -Descending |
        Select-Object -First 1
    if ($managedNode) { $env:PATH = "$($managedNode.FullName);$env:PATH" }
}
# Always call npm.cmd, never bare 'npm': on machines where the Node folder is on
# PATH, 'npm' resolves to npm.ps1 (a PowerShell script). A script call does NOT
# set $LASTEXITCODE, so a failed build would look successful (and under
# Set-StrictMode it throws on the undefined variable instead). npm.cmd is a real
# native batch wrapper -> exit code behaves.
$npmCmd = Get-Command npm.cmd -ErrorAction SilentlyContinue
if (-not $npmCmd) {
    $npmCmd = Get-Command npm -CommandType Application -ErrorAction SilentlyContinue
}
if (-not $npmCmd) {
    throw 'npm not found. Add Node 22 to PATH, or drop a portable Node under the managed node root.'
}
$npm = $npmCmd.Source
# Rust: a fresh rustup install is not on PATH until the terminal is reopened,
# so fall back to the default install location before giving up.
if ($Target -ne 'Frontend') {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -and (Test-Path (Join-Path $cargoBin 'cargo.exe'))) {
        $env:PATH = "$cargoBin;$env:PATH"
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'cargo not found. Install the Rust MSVC toolchain, or add %USERPROFILE%\.cargo\bin to PATH.'
    }
}

$cfg = if ($DebugBuild) { 'debug' } else { 'release' }
$version = (Get-Content (Join-Path $tauri 'tauri.conf.json') -Raw | ConvertFrom-Json).version

function Invoke-Native {
    param([string]$Label, [scriptblock]$Body)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Write-Host "==> $Label" -ForegroundColor Cyan
    # Reset so a non-native call cannot leave a stale code behind
    # (and so Set-StrictMode never trips on an undefined variable).
    $global:LASTEXITCODE = 0
    $old = $ErrorActionPreference
    # cargo/tsc write progress to stderr; with 'Stop' that becomes a fatal
    # NativeCommandError even on success. Switch to Continue and rely on exit code.
    $ErrorActionPreference = 'Continue'
    & $Body
    $code = $LASTEXITCODE
    $ErrorActionPreference = $old
    $sw.Stop()
    if ($code -ne 0) { throw "$Label failed (exit $code)" }
    Write-Host ("    ok  {0:n1}s" -f $sw.Elapsed.TotalSeconds) -ForegroundColor DarkGray
}

# ---- clean -----------------------------------------------------------------
if ($Clean) {
    Write-Host '==> clean build area' -ForegroundColor Cyan
    foreach ($d in @($distDir, $targetDir)) {
        if (Test-Path $d) { Get-ChildItem -LiteralPath $d -Force | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

Push-Location $proj
try {
    # ---- frontend: tsc --noEmit && vite build -> build\dist -----------------
    if ($Target -in @('All', 'Frontend')) {
        Invoke-Native 'frontend  (tsc --noEmit && vite build -> build\dist)' { & $npm run build }
    }

    # ---- backend: cargo + nsis -> build\target, then archive ----------------
    if ($Target -in @('All', 'Backend')) {
        $npmArgs = @('run', 'tauri:build')
        if ($DebugBuild) { $npmArgs += @('--', '--debug') }
        Invoke-Native "backend   (tauri build [$cfg] -> build\target)" { & $npm @npmArgs }
    }
}
finally {
    Pop-Location
}

# ---- archive installer -----------------------------------------------------
$bundleRoot = Join-Path $targetDir $cfg
$setup = Get-ChildItem -Path (Join-Path $bundleRoot 'bundle') -Recurse -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($setup) {
    $dest = Join-Path $relDir $setup.Name
    Copy-Item -LiteralPath $setup.FullName -Destination $dest -Force
    $mib = [math]::Round($setup.Length / 1MB, 2)
    Write-Host ''
    Write-Host '---------------- build summary ----------------' -ForegroundColor Green
    Write-Host ("  version    : {0}" -f $version)
    Write-Host ("  profile    : {0}" -f $cfg)
    Write-Host ("  frontend   : build\dist")
    Write-Host ("  target dir : build\target\{0}" -f $cfg)
    Write-Host ("  installer  : build\release\{0}  ({1} MiB)" -f $setup.Name, $mib)
    Write-Host ("  raw size   : {0:n0} bytes" -f $setup.Length)
    Write-Host '-----------------------------------------------' -ForegroundColor Green
}
else {
    Write-Host 'No installer produced (frontend-only run?).' -ForegroundColor Yellow
}

if ($logPath) { try { Stop-Transcript | Out-Null } catch { } }
