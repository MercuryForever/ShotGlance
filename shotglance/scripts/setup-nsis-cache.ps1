# setup-nsis-cache.ps1 -- deploy the vendored NSIS toolset into Tauri's tool cache
# offline, so `npm run tauri:build` never tries to download it.
#
# Why this exists:
#   Tauri validates NSIS_REQUIRED_FILES under %LOCALAPPDATA%\tauri\NSIS and, if ANY
#   one is missing, deletes the whole directory and re-downloads from GitHub.
#   Two details are easy to get wrong:
#     1. The zip's top-level folder `nsis-3.13/` must be FLATTENED into `NSIS\`
#        (Tauri itself does rename("nsis-3.13", "NSIS") after extraction).
#     2. The plugin dll must live in `Plugins\x86-unicode\additional\`, not
#        directly under `x86-unicode\`.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\setup-nsis-cache.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\setup-nsis-cache.ps1 -Verify

[CmdletBinding()]
param(
    [string]$ZipPath,
    [string]$DllPath,
    [string]$CacheDir,
    [switch]$Verify
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjDir   = Split-Path -Parent $ScriptDir
$VendorDir = Join-Path $ScriptDir 'vendor'

if (-not $ZipPath) { $ZipPath = Join-Path $VendorDir 'nsis-3.13.zip' }
if (-not $DllPath) { $DllPath = Join-Path $VendorDir 'nsis_tauri_utils.dll' }
if (-not $CacheDir) {
    if ($env:LOCALAPPDATA) { $CacheDir = Join-Path $env:LOCALAPPDATA 'tauri\NSIS' }
    else { throw 'LOCALAPPDATA is not set; pass -CacheDir explicitly.' }
}

# The 13 files Tauri checks (NSIS_REQUIRED_FILES, windows target).
$RequiredFiles = @(
    'makensis.exe',
    'Bin\makensis.exe',
    'Stubs\lzma-x86-unicode',
    'Stubs\lzma_solid-x86-unicode',
    'Plugins\x86-unicode\additional\nsis_tauri_utils.dll',
    'Include\MUI2.nsh',
    'Include\FileFunc.nsh',
    'Include\x64.nsh',
    'Include\nsDialogs.nsh',
    'Include\WinMessages.nsh',
    'Include\Win\COM.nsh',
    'Include\Win\Propkey.nsh',
    'Include\Win\RestartManager.nsh'
)

function Test-Cache {
    param([string]$Dir)
    $missing = @()
    foreach ($rel in $RequiredFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $Dir $rel))) { $missing += $rel }
    }
    return $missing
}

if ($Verify) {
    Write-Host ''
    Write-Host ('    verifying ' + $CacheDir)
    $missing = Test-Cache -Dir $CacheDir
    if ($missing.Count -eq 0) {
        Write-Host '    OK  all 13 required files present'
        exit 0
    }
    Write-Host ('    MISSING ' + $missing.Count + ' file(s):')
    foreach ($m in $missing) { Write-Host ('      - ' + $m) }
    exit 1
}

Write-Host ''
Write-Host '    deploying vendored NSIS to Tauri cache'

foreach ($p in @($ZipPath, $DllPath)) {
    if (-not (Test-Path -LiteralPath $p)) { throw ('missing vendor file: ' + $p) }
}

$ZipHash = (Get-FileHash -LiteralPath $ZipPath -Algorithm SHA1).Hash
$DllHash = (Get-FileHash -LiteralPath $DllPath -Algorithm SHA1).Hash
if ($ZipHash -ne 'DB14F8AF2EA346B786C6AE19343F61AAF5E09B8E') {
    Write-Warning ('nsis zip SHA1 differs from the constant tauri-bundler expects: ' + $ZipHash)
}
if ($DllHash -ne '75197FEE3C6A814FE035788D1C34EAD39349B860') {
    Write-Warning ('nsis_tauri_utils.dll SHA1 differs from the constant tauri-bundler expects: ' + $DllHash)
}

$tmp = Join-Path $env:TEMP ('sg-nsis-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
try {
    Expand-Archive -LiteralPath $ZipPath -DestinationPath $tmp -Force

    # The archive holds a single top-level folder; flatten it into the cache root.
    $entries = @(Get-ChildItem -LiteralPath $tmp -Force)
    $src = $tmp
    if ($entries.Count -eq 1 -and $entries[0].PSIsContainer) { $src = $entries[0].FullName }

    if (Test-Path -LiteralPath $CacheDir) {
        [IO.Directory]::Delete($CacheDir, $true)
    }
    New-Item -ItemType Directory -Path $CacheDir -Force | Out-Null

    Get-ChildItem -LiteralPath $src -Force | ForEach-Object {
        Move-Item -LiteralPath $_.FullName -Destination $CacheDir -Force
    }

    $additional = Join-Path $CacheDir 'Plugins\x86-unicode\additional'
    New-Item -ItemType Directory -Path $additional -Force | Out-Null
    Copy-Item -LiteralPath $DllPath -Destination $additional -Force

    $missing = Test-Cache -Dir $CacheDir
    if ($missing.Count -gt 0) {
        Write-Host ('    FAILED, still missing ' + $missing.Count + ' file(s):')
        foreach ($m in $missing) { Write-Host ('      - ' + $m) }
        exit 1
    }
    Write-Host ('    OK  deployed to ' + $CacheDir + ' (13/13 files)')
    Write-Host ''
    Write-Host '    next: clear inherited proxy vars, then run `npm run tauri:build`'
}
finally {
    if (Test-Path -LiteralPath $tmp) {
        [IO.Directory]::Delete($tmp, $true)
    }
}
