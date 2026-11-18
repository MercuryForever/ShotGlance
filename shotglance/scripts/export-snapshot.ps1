#Requires -Version 5.1
<#
  export-snapshot.ps1 - build a portable, self-describing snapshot of the
  ShotGlance project so it can be restored on another machine.

  Kept pure ASCII on purpose: PowerShell 5.1 reads BOM-less UTF-8 as ANSI(GBK)
  and non-ASCII comments can silently break the param block.

  What is included is an explicit allow-list (not a wildcard), so a stray
  multi-GB cache can never sneak in. Excluded-but-reproducible directories
  (node_modules, target, gen, dist) are listed in the report.

  IMPORTANT: when you add a source file (a new module, a new asset), add it to
  the allow-list below. A missing entry fails silently - the snapshot still
  builds, it just does not contain your new file. The 2026-11-09 snapshot
  lost the entire translate/ module this way before the list was fixed.

  Usage:
    powershell -ExecutionPolicy Bypass -File scripts\export-snapshot.ps1
    powershell -ExecutionPolicy Bypass -File scripts\export-snapshot.ps1 -NoInstaller
#>
param(
    [string]$OutDir = '',
    [string]$Name = '',
    [switch]$NoInstaller
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjDir = Split-Path -Parent $ScriptDir          # ...\shotglance
$RootDir = Split-Path -Parent $ProjDir            # workspace root

if ([string]::IsNullOrEmpty($OutDir)) { $OutDir = $RootDir }
if ([string]::IsNullOrEmpty($Name)) { $Name = 'shotglance-snapshot-' + (Get-Date -Format 'yyyyMMdd-HHmm') }

$Stage = Join-Path $env:TEMP ('sg-snapshot-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$ZipPath = Join-Path $OutDir ($Name + '.zip')
$ReportPath = Join-Path $OutDir ($Name + '-report.txt')

# Collect the report in memory and write it once at the end: Add-Content with an
# empty string does not emit a blank line on PS 5.1, which silently glues the
# next entry onto the previous one ("...driver installer)RESULT").
$ReportLines = New-Object System.Collections.Generic.List[string]

function Write-Log {
    param([string]$Text)
    [void]$script:ReportLines.Add($Text)
}

function Save-Report {
    Set-Content -LiteralPath $ReportPath -Value $ReportLines -Encoding UTF8
}

function Copy-Rel {
    param([string]$RelPath)
    $src = Join-Path $RootDir $RelPath
    if (-not (Test-Path -LiteralPath $src)) {
        Write-Log ("MISSING  " + $RelPath)
        return 0
    }
    $dst = Join-Path $Stage $RelPath
    $dstDir = Split-Path -Parent $dst
    if (-not (Test-Path -LiteralPath $dstDir)) { New-Item -ItemType Directory -Path $dstDir -Force | Out-Null }
    if ((Get-Item -LiteralPath $src).PSIsContainer) {
        Copy-Item -LiteralPath $src -Destination $dst -Recurse -Force
    } else {
        Copy-Item -LiteralPath $src -Destination $dst -Force
    }
    return 1
}

# ---------------------------------------------------------------- allow-list
# Paths are relative to the workspace root (the parent of shotglance\).
$IncludeFiles = @(
    # --- workspace level (Git area: version control + repo home)
    'README.md'
    'CHANGELOG.md'
    'LICENSE'
    '.gitignore'
    '.gitattributes'
    'build.ps1'
    'verify-env.ps1'
    # --- frontend
    'shotglance/.gitignore'
    'shotglance/README.md'
    'shotglance/HANDOFF.md'
    'shotglance/ENV-MIGRATION.md'
    'shotglance/index.html'
    'shotglance/package.json'
    'shotglance/package-lock.json'
    'shotglance/tsconfig.json'
    'shotglance/vite.config.ts'
    'shotglance/src/main.ts'
    'shotglance/src/styles.css'
    # --- build config
    'shotglance/src-tauri/Cargo.toml'
    'shotglance/src-tauri/Cargo.lock'
    'shotglance/src-tauri/build.rs'
    'shotglance/src-tauri/tauri.conf.json'
    'shotglance/src-tauri/.cargo/config.toml'
    # --- backend sources
    'shotglance/src-tauri/src/lib.rs'
    'shotglance/src-tauri/src/main.rs'
    'shotglance/src-tauri/src/capture/mod.rs'
    'shotglance/src-tauri/src/ocr/mod.rs'
    'shotglance/src-tauri/src/ocr/det.rs'
    'shotglance/src-tauri/src/ocr/rec.rs'
    'shotglance/src-tauri/src/translate/mod.rs'
    'shotglance/src-tauri/src/translate/online.rs'
    'shotglance/src-tauri/src/translate/offline.rs'
    # --- runtime assets (cannot be re-downloaded cheaply)
    'shotglance/src-tauri/models/det.onnx'
    'shotglance/src-tauri/models/rec.onnx'
    'shotglance/src-tauri/models/dict.txt'
    'shotglance/src-tauri/vendor/onnxruntime.dll'
    'shotglance/src-tauri/assets/mt/tokens.tsv'
    # --- tests / benchmarks
    'shotglance/src-tauri/tests/ocr-sample.png'
    # --- scripts
    'shotglance/scripts/measure-memory.ps1'
    'shotglance/scripts/restore-nsis-cache.ps1'
    'shotglance/scripts/make-ocr-sample.ps1'
    'shotglance/scripts/export-snapshot.ps1'
    'shotglance/scripts/export-env-bundle.ps1'
    'shotglance/scripts/restore-env-bundle.ps1'
    'shotglance/scripts/gen_icon.py'
    'shotglance/scripts/mt/README.md'
    'shotglance/scripts/mt/ref.py'
    'shotglance/scripts/mt/cmp.py'
    'shotglance/scripts/mt/export_vocab.py'
    'shotglance/scripts/mt/merge_vocab.py'
    'shotglance/scripts/mt/transform_lmhead.py'
    'shotglance/scripts/mt/verify_lmhead.py'
    'shotglance/scripts/mt/probe_lmhead.py'
    'shotglance/scripts/mt/probe_pattern.py'
    'shotglance/scripts/mt/profile_ops.py'
    'shotglance/scripts/mt/bench_decode.py'
    'shotglance/scripts/vendor/nsis-3.13.zip'
    'shotglance/scripts/vendor/nsis_tauri_utils.dll'
)

$IncludeDirs = @(
    'shotglance/src-tauri/capabilities'
    'shotglance/src-tauri/icons'
    '.workbuddy/memory'
)

# ---------------------------------------------------------------- excluded
$Excluded = @(
    'shotglance/node_modules        (~62 MB, npm ci)'
    'shotglance/src-tauri/target    (~7.6 GB, npm run tauri:build)'
    'shotglance/src-tauri/gen       (~313 KB, regenerated by tauri-build)'
    'shotglance/dist                (~204 KB, npm run build)'
    '.setup                         (installer downloads, re-downloadable)'
    'NVIDIA Corporation             (empty dirs left by the driver installer)'
)

if (Test-Path -LiteralPath $Stage) { Remove-Item -LiteralPath $Stage -Recurse -Force }
New-Item -ItemType Directory -Path $Stage -Force | Out-Null

# $env:TEMP can be the 8.3 short form (C:\Users\ADMINI~1\...) while Get-ChildItem
# reports long names (C:\Users\Administrator\...). Subtracting the short-form
# length leaves a garbage prefix on every archive entry and manifest line.
# Always measure against the resolved path.
$StageFull = (Get-Item -LiteralPath $Stage).FullName

function Get-RelPath {
    param([string]$FullName)
    if ($FullName.StartsWith($StageFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $FullName.Substring($StageFull.Length).TrimStart('\', '/')
    }
    return $FullName
}

Write-Log 'ShotGlance portable snapshot'
Write-Log ('generated  : ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
Write-Log ('source     : ' + $RootDir)
Write-Log ''
Write-Log 'INCLUDED'
$copied = 0
foreach ($p in $IncludeFiles) { $copied += Copy-Rel $p }
foreach ($p in $IncludeDirs) { $copied += Copy-Rel $p }
Write-Log ("  entries copied: " + $copied)

if (-not $NoInstaller) {
    # 安装包现在住在仓库根的构建区 build\release（归档）与 build\target\release\bundle\nsis
    # （新产物）。两处都用通配找，版本号升级后本脚本无需改。
    $candidates = @()
    foreach ($dir in @(
            (Join-Path $RootDir 'build\release'),
            (Join-Path $RootDir 'build\target\release\bundle\nsis')
        )) {
        if (Test-Path -LiteralPath $dir) {
            $candidates += @(Get-ChildItem -LiteralPath $dir -Filter '*-setup.exe' -File -ErrorAction SilentlyContinue |
                             Sort-Object LastWriteTime -Descending | ForEach-Object { $_.FullName })
        }
    }
    $setup = $null
    foreach ($c in $candidates) { if (Test-Path -LiteralPath $c) { $setup = $c; break } }
    if ($setup) {
        $rel = 'release/' + (Split-Path -Leaf $setup)
        $dst = Join-Path $Stage $rel
        $dstDir = Split-Path -Parent $dst
        if (-not (Test-Path -LiteralPath $dstDir)) { New-Item -ItemType Directory -Path $dstDir -Force | Out-Null }
        Copy-Item -LiteralPath $setup -Destination $dst -Force
        Write-Log ('  installer: ' + $rel + '  (from ' + $setup + ')')
    } else {
        Write-Log '  installer: NOT FOUND (run npm run tauri:build first)'
    }
}

Write-Log ''
Write-Log 'EXCLUDED (reproducible)'
foreach ($e in $Excluded) { Write-Log ('  ' + $e) }

# ---------------------------------------------------------------- manifest
$manifest = Join-Path $Stage 'MANIFEST.txt'
$lines = New-Object System.Collections.Generic.List[string]
$lines.Add('ShotGlance snapshot manifest')
$lines.Add(('generated: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')))
$lines.Add('')
$lines.Add('SHA256                                                            bytes  path')
$total = 0
Get-ChildItem -LiteralPath $Stage -Recurse -File | ForEach-Object {
    $rel = Get-RelPath $_.FullName
    $h = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()
    $total += $_.Length
    $lines.Add(('{0}  {1,9}  {2}' -f $h, $_.Length, $rel))
}
$lines.Add('')
$lines.Add(('files (excl. this manifest): ' + ($lines.Count - 5)))
$lines.Add(('bytes: ' + $total))
Set-Content -LiteralPath $manifest -Value $lines -Encoding UTF8

# ---------------------------------------------------------------- zip
# Written through a FileStream opened with FileMode.Create so re-running over
# an existing archive truncates it in place. Two things must be avoided here:
#   - [ZipFile]::CreateFromDirectory throws IOException when the target exists
#     (it does NOT overwrite, contrary to what an earlier comment claimed);
#   - Remove-Item on a user file is intercepted by the sandbox safe-delete shim
#     and aborts the script, so "delete then create" is not an option either.
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$fs = [System.IO.File]::Open($ZipPath, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
try {
    $archive = New-Object System.IO.Compression.ZipArchive(
        $fs,
        [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        Get-ChildItem -LiteralPath $Stage -Recurse -File | ForEach-Object {
            $rel = (Get-RelPath $_.FullName).Replace('\', '/')
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive,
                $_.FullName,
                $rel,
                [System.IO.Compression.CompressionLevel]::Optimal)
        }
    } finally {
        $archive.Dispose()
    }
} finally {
    $fs.Dispose()
}

$zip = Get-Item -LiteralPath $ZipPath
Write-Log ''
Write-Log 'RESULT'
Write-Log ('  zip      : ' + $ZipPath)
Write-Log ('  zip size : ' + [math]::Round($zip.Length / 1MB, 2) + ' MB')
Write-Log ('  raw size : ' + [math]::Round($total / 1MB, 2) + ' MB')
Write-Log ('  files    : ' + (Get-ChildItem -LiteralPath $Stage -Recurse -File | Measure-Object).Count + '  (incl. MANIFEST.txt)')
Write-Log ''
Write-Log 'Restore on another machine: see shotglance/HANDOFF.md'
Save-Report

Remove-Item -LiteralPath $Stage -Recurse -Force
Write-Output ('zip: ' + $ZipPath)
Write-Output ('report: ' + $ReportPath)
