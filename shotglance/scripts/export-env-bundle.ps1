#Requires -Version 5.1
<#
  export-env-bundle.ps1 - capture the LOCAL BUILD ENVIRONMENT into a plain
  folder that can be copied to a USB disk.

  Why this exists
  ---------------
  The 2026-11-12 setup on a fresh machine took ~3 hours, and almost all of it
  was downloads: rust-lang served 71 KB/s, VS CDN 200 KB/s. Nothing that was
  downloaded there is machine-specific - it is all cacheable. This script
  packs it once; restore-env-bundle.ps1 unpacks it in minutes.

  Bundle layout
  -------------
    env-bundle\
      MANIFEST.txt                  versions, per-part sizes, key hashes
      installers\                   bootstrap exes (no network needed later)
      toolchain\rustup\             %USERPROFILE%\.rustup   (591 MB)
      toolchain\cargo-bin\          %USERPROFILE%\.cargo\bin (shims)
      toolchain\cargo-registry\     .cargo\registry cache+index (79 MB)
      node_modules\                 shotglance\node_modules  (62 MB)
      target\                       src-tauri\target (only with -WithTarget)
      vs-layout\                    VS offline layout (only with -WithVsLayout)

  Deliberately NOT included
  -------------------------
    - .cargo\registry\src   (314 MB). Cargo re-extracts it from the cached
      .crate files for free, so shipping it is pure waste.
    - MSVC / Windows SDK. Those are installed by the VS installer only; use
      -WithVsLayout to build an offline layout, otherwise install online.
    - target\  by default: it embeds absolute paths, so it is only reusable
      when the workspace sits at the exact same path on the new machine.

  NOTE ON ENCODING
  ----------------
  This file is pure ASCII on purpose. PowerShell 5.1 reads BOM-less UTF-8 as
  ANSI (GBK) and non-ASCII text can corrupt the param block silently.

  Usage
  -----
    powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1
    powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -WithTarget
    powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -WithVsLayout
    powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -OutDir D:\sg-env
#>
param(
    [string]$OutDir = '',
    [switch]$WithTarget,
    [switch]$WithVsLayout,
    [switch]$SkipNodeModules,
    [switch]$SkipRust
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjDir = Split-Path -Parent $ScriptDir      # ...\shotglance
$RootDir = Split-Path -Parent $ProjDir        # workspace root (parent of shotglance)

if ([string]::IsNullOrEmpty($OutDir)) { $OutDir = Join-Path $RootDir 'env-bundle' }

$Manifest = Join-Path $OutDir 'MANIFEST.txt'
$Lines = New-Object System.Collections.Generic.List[string]
$Parts = New-Object System.Collections.Generic.List[string]
$ShimNames = New-Object System.Collections.Generic.List[string]

function Log {
    param([string]$Text)
    Write-Host $Text
    [void]$script:Lines.Add($Text)
}

# robocopy is used instead of Copy-Item: it is an order of magnitude faster on
# many-small-files trees (node_modules has 455 files, the cargo registry 12306)
# and it reports a real exit code. 0-7 mean success, >=8 means failure.
function Copy-Tree {
    param([string]$Src, [string]$Dst, [string]$Label)

    if (-not (Test-Path -LiteralPath $Src)) {
        Log ("  SKIP   " + $Label + "   (not found: " + $Src + ")")
        return $false
    }
    $parent = Split-Path -Parent $Dst
    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }

    # /E merge, no /MIR: never delete inside the bundle from a stale run.
    $rcArgs = @($Src, $Dst, '/E', '/MT:16', '/R:1', '/W:1', '/NFL', '/NDL', '/NJH', '/NJS', '/NP')
    & robocopy @rcArgs | Out-Null
    $rc = $LASTEXITCODE
    if ($rc -ge 8) { throw ("robocopy failed for " + $Label + " (exit " + $rc + ")") }

    # Measure-Object needs -Property Length -Sum, otherwise there is no Sum at all
    # and every part reports "0 MB" while the file count looks fine.
    $files = (Get-ChildItem -LiteralPath $Dst -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum)
    $mb = 0
    if ($files.Sum) { $mb = [math]::Round($files.Sum / 1MB, 1) }
    Log ("  OK     " + $Label + "   " + $files.Count + " files, " + $mb + " MB")
    [void]$script:Parts.Add(('{0,-22} {1,7} files {2,9} MB' -f $Label, $files.Count, $mb))
    return $true
}

function Measure-Mb {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $m = Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum
    if (-not $m.Sum) { return 0 }
    return [math]::Round($m.Sum / 1MB, 1)
}

# ------------------------------------------------------------------ prologue
Write-Host ''
Write-Host '=== ShotGlance environment bundle export ===' -ForegroundColor Cyan
Write-Host ('    target: ' + $OutDir)
Write-Host ''

if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }

$RustupHome = Join-Path $env:USERPROFILE '.rustup'
$CargoHome  = Join-Path $env:USERPROFILE '.cargo'

# ------------------------------------------------------------ version probe
$rustcV = 'not found'
$cargoV = 'not found'
$cargoExe = Join-Path $CargoHome 'bin\cargo.exe'
if (Test-Path -LiteralPath $cargoExe) {
    $rustcV = (& (Join-Path $CargoHome 'bin\rustc.exe') -V 2>&1) -join ' '
    $cargoV = (& $cargoExe -V 2>&1) -join ' '
}
$nodeV = (node --version 2>&1) -join ' '
$npmV  = (npm  --version 2>&1) -join ' '

$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$vsPath = ''
$vsVer  = ''
if (Test-Path -LiteralPath $vswhere) {
    $vsPath = (& $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>&1) -join ''
    if ($vsPath) { $vsVer = (& $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationVersion 2>&1) -join '' }
}
$msvcVer = ''
if ($vsPath) {
    $m = Get-ChildItem (Join-Path $vsPath 'VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
    if ($m) { $msvcVer = $m.Name }
}
$sdkVer = ''
$sdkRoot = 'C:\Program Files (x86)\Windows Kits\10\Include'
if (Test-Path -LiteralPath $sdkRoot) {
    $s = Get-ChildItem $sdkRoot -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
    if ($s) { $sdkVer = $s.Name }
}

# ------------------------------------------------------------------- copy
Write-Host '[1/4] toolchain' -ForegroundColor Yellow

if (-not $SkipRust) {
    Copy-Tree -Src $RustupHome -Dst (Join-Path $OutDir 'toolchain\rustup') -Label 'rustup-home' | Out-Null

    # %USERPROFILE%\.cargo\bin is NOT copied with robocopy: 13 of its 14 entries
    # are symlinks to rustup.exe (12.8 MB each). robocopy follows them and writes
    # 14 full copies = 172 MB instead of 13 MB. Copy the real exe once and record
    # the shim names, so restore can rebuild them as hard links.
    $cargoBin = Join-Path $CargoHome 'bin'
    if (Test-Path -LiteralPath $cargoBin) {
        $binDst = Join-Path $OutDir 'toolchain\cargo-bin'
        if (-not (Test-Path -LiteralPath $binDst)) { New-Item -ItemType Directory -Path $binDst -Force | Out-Null }
        $rustupExe = Join-Path $cargoBin 'rustup.exe'
        if (Test-Path -LiteralPath $rustupExe) {
            Copy-Item -LiteralPath $rustupExe -Destination (Join-Path $binDst 'rustup.exe') -Force
            $sz = (Get-Item -LiteralPath (Join-Path $binDst 'rustup.exe')).Length
            Log ('  OK     cargo-bin   rustup.exe only, ' + [math]::Round($sz / 1MB, 1) + ' MB (shims rebuilt on restore)')
            [void]$Parts.Add(('{0,-22} {1,7} files {2,9} MB' -f 'cargo-bin', 1, [math]::Round($sz / 1MB, 1)))
        } else {
            Log '  WARN   cargo-bin   rustup.exe missing - rustup cannot be managed after restore'
        }
        foreach ($f in [System.IO.Directory]::GetFiles($cargoBin)) {
            $n = Split-Path -Leaf $f
            if ($n -ne 'rustup.exe') { [void]$script:ShimNames.Add($n) }
        }
        Log ('  OK     cargo-bin   ' + $ShimNames.Count + ' shim name(s) recorded')
    }

    # cache (.crate archives) + index only. src\ is rebuilt from cache by cargo.
    $reg = Join-Path $CargoHome 'registry'
    if (Test-Path -LiteralPath $reg) {
        New-Item -ItemType Directory -Path (Join-Path $OutDir 'toolchain\cargo-registry') -Force | Out-Null
        foreach ($sub in @('cache', 'index')) {
            $s = Join-Path $reg $sub
            if (Test-Path -LiteralPath $s) {
                Copy-Tree -Src $s -Dst (Join-Path $OutDir ('toolchain\cargo-registry\' + $sub)) -Label ('cargo-registry/' + $sub) | Out-Null
            }
        }
    }
    $cfg = Join-Path $CargoHome 'config.toml'
    if (Test-Path -LiteralPath $cfg) {
        New-Item -ItemType Directory -Path (Join-Path $OutDir 'toolchain\cargo-home') -Force | Out-Null
        Copy-Item -LiteralPath $cfg -Destination (Join-Path $OutDir 'toolchain\cargo-home\config.toml') -Force
        Log '  OK     cargo config.toml'
    }
}

Write-Host ''
Write-Host '[2/4] node_modules' -ForegroundColor Yellow
if (-not $SkipNodeModules) {
    Copy-Tree -Src (Join-Path $ProjDir 'node_modules') -Dst (Join-Path $OutDir 'node_modules') -Label 'node_modules' | Out-Null
}

Write-Host ''
Write-Host '[3/4] build artifacts' -ForegroundColor Yellow
if ($WithTarget) {
    Copy-Tree -Src (Join-Path $ProjDir 'src-tauri\target') -Dst (Join-Path $OutDir 'target') -Label 'target' | Out-Null
    Log '         NOTE: target embeds absolute paths; it only helps at the exact same workspace path.'
} else {
    Log '  SKIP   target  (use -WithTarget; only useful at the identical workspace path)'
}

Write-Host ''
Write-Host '[4/4] installers' -ForegroundColor Yellow

$InstDir = Join-Path $OutDir 'installers'
if (-not (Test-Path -LiteralPath $InstDir)) { New-Item -ItemType Directory -Path $InstDir -Force | Out-Null }

# Keep the two bootstrap exes so a machine with no network at all can still
# start from the bundle instead of guessing URLs.
$cl = Join-Path $env:USERPROFILE 'Downloads\_envsetup'
$boots = @(
    @{ Src = (Join-Path $cl 'rustup-init.exe');        Name = 'rustup-init.exe' },
    @{ Src = (Join-Path $cl 'vs_BuildTools.exe');      Name = 'vs_BuildTools.exe' }
)
foreach ($b in $boots) {
    if (Test-Path -LiteralPath $b.Src) {
        Copy-Item -LiteralPath $b.Src -Destination (Join-Path $InstDir $b.Name) -Force
        $len = (Get-Item -LiteralPath (Join-Path $InstDir $b.Name)).Length
        Log ('  OK     ' + $b.Name + '   ' + [math]::Round($len / 1MB, 1) + ' MB')
    } else {
        Log ('  SKIP   ' + $b.Name + '   (not found in ' + $cl + ')')
    }
}

# NSIS is already vendored inside the repo; just assert it is there.
$nsisZip = Join-Path $ProjDir 'scripts\vendor\nsis-3.13.zip'
if (Test-Path -LiteralPath $nsisZip) {
    Log ('  OK     nsis vendor zip present (restored by restore-nsis-cache.ps1)')
} else {
    Log '  WARN   scripts\vendor\nsis-3.13.zip missing - packaging will try the network'
}

# --------------------------------------------------------------- VS layout
if ($WithVsLayout) {
    Write-Host ''
    Write-Host '[5/5] VS Build Tools offline layout (this downloads ~2.5 GB, be patient)' -ForegroundColor Yellow
    $layout = Join-Path $OutDir 'vs-layout'
    $vsBoot = Join-Path $InstDir 'vs_BuildTools.exe'
    if (-not (Test-Path -LiteralPath $vsBoot)) {
        Log '  FAIL   vs_BuildTools.exe not available; cannot build a layout'
    } else {
        # Same component set that was verified on 2026-11-12. Do not trim here:
        # a smaller layout is a different, unverified toolchain.
        $layArgs = @(
            '--layout', $layout,
            '--add', 'Microsoft.VisualStudio.Workload.VCTools',
            '--add', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
            '--includeRecommended',
            '--lang', 'en-US',
            '--quiet', '--wait', '--norestart'
        )
        Log ('  running: vs_BuildTools.exe ' + ($layArgs -join ' '))
        $p = Start-Process -FilePath $vsBoot -ArgumentList $layArgs -Wait -PassThru
        Log ('  layout exit code: ' + $p.ExitCode)
        $mb = Measure-Mb $layout
        Log ('  OK     vs-layout   ' + $mb + ' MB')
        [void]$Parts.Add(('{0,-22} {1,7}       {2,9} MB' -f 'vs-layout', '', $mb))
    }
}

# ---------------------------------------------------------------- manifest
$SettingsToml = Join-Path $OutDir 'toolchain\rustup\settings.toml'
$defaultTc = ''
if (Test-Path -LiteralPath $SettingsToml) {
    $l = Select-String -LiteralPath $SettingsToml -Pattern '^default_toolchain' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($l) { $defaultTc = ($l.Line -replace '^default_toolchain\s*=\s*', '').Trim('"', "'", ' ') }
}

$totalMb = Measure-Mb $OutDir

Log ''
Log 'RESULT'
Log ('  bundle   : ' + $OutDir)
Log ('  total    : ' + $totalMb + ' MB')
Log ''
Log 'Copy the whole folder to a USB disk / network share, together with the'
Log 'source snapshot zip. Restore with scripts\restore-env-bundle.ps1 -BundleDir <path>.'

$header = New-Object System.Collections.Generic.List[string]
$header.Add('ShotGlance environment bundle - MANIFEST')
$header.Add(('generated  : ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')))
$header.Add(('host       : ' + $env:COMPUTERNAME))
$header.Add(('workspace  : ' + $RootDir))
$header.Add(('bundleSize : ' + $totalMb + ' MB'))
$header.Add('')
$header.Add('[versions]  (restore-env-bundle.ps1 compares these after restoring)')
$header.Add(('rustc      : ' + $rustcV))
$header.Add(('cargo      : ' + $cargoV))
$header.Add(('default_tc : ' + $defaultTc))
$header.Add(('rustup_tc  : stable-x86_64-pc-windows-msvc'))
$header.Add(('node       : ' + $nodeV))
$header.Add(('npm        : ' + $npmV))
$header.Add(('vs_path    : ' + $vsPath))
$header.Add(('vs_version : ' + $vsVer))
$header.Add(('msvc       : ' + $msvcVer))
$header.Add(('winsdk     : ' + $sdkVer))
$header.Add(('vs_layout  : ' + [string]$WithVsLayout))
$header.Add(('has_target : ' + [string]$WithTarget))
$header.Add('')
$header.Add('[parts]')
foreach ($p in $Parts) { $header.Add('  ' + $p) }
$header.Add('')
$header.Add('[cargo-bin-shims]  (restore recreates these as hard links to rustup.exe)')
foreach ($s in $ShimNames) { $header.Add('  ' + $s) }
$header.Add('')
$header.Add('[hashes]  (single-file assets; trees are covered by size + file count)')

$hashLines = New-Object System.Collections.Generic.List[string]
Get-ChildItem -LiteralPath $InstDir -File -ErrorAction SilentlyContinue | ForEach-Object {
    $h = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()
    $hashLines.Add(('  ' + $h + '  ' + $_.Length.ToString().PadLeft(10) + '  installers\' + $_.Name))
}
foreach ($h in $hashLines) { $header.Add($h) }

$header.Add('')
$header.Add('NOT INCLUDED ON PURPOSE')
$header.Add('  .cargo\registry\src  314 MB, re-extracted from cache\*.crate by cargo')
$header.Add('  MSVC + Windows SDK   install via VS installer (use vs-layout if present)')
$header.Add('')
$header.Add('PER-PART RESTORE HINTS')
$header.Add('  toolchain\rustup        -> %USERPROFILE%\.rustup')
$header.Add('  toolchain\cargo-bin     -> %USERPROFILE%\.cargo\bin')
$header.Add('  toolchain\cargo-home    -> %USERPROFILE%\.cargo')
$header.Add('  toolchain\cargo-registry-> %USERPROFILE%\.cargo\registry')
$header.Add('  node_modules            -> shotglance\node_modules')
$header.Add('  target                  -> shotglance\src-tauri\target (same path only)')

Set-Content -LiteralPath $Manifest -Value $header -Encoding UTF8

Write-Host ''
Write-Host ('manifest: ' + $Manifest) -ForegroundColor Green
Write-Host ('total   : ' + $totalMb + ' MB') -ForegroundColor Green
