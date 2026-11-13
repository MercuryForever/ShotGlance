#Requires -Version 5.1
<#
  restore-env-bundle.ps1 - unpack an env-bundle produced by
  export-env-bundle.ps1, so a new machine reaches "cargo check passes" in
  minutes instead of hours.

  What it does, in order
  ----------------------
    1. read MANIFEST.txt, show the recorded toolchain versions
    2. restore %USERPROFILE%\.rustup           (toolchains + settings.toml)
    3. restore %USERPROFILE%\.cargo\bin        (rustc/cargo shims)
    4. restore %USERPROFILE%\.cargo\registry   (crate cache + index)
    5. restore shotglance\node_modules
    6. restore src-tauri\target  - only when the workspace path matches the
       path recorded in the manifest, otherwise the incremental cache is
       worthless (Rust dep-info files embed absolute paths)
    7. check MSVC + Windows SDK; install from vs-layout if present, else
       print the exact command to run
    8. verify: rustc -V, cargo -V, and optionally cargo check

  ENCODING: pure ASCII on purpose, see export-env-bundle.ps1.

  Usage
  -----
    powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env
    powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -WithVsInstall
    powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -AddToPath -Check
#>
param(
    [string]$BundleDir = '',
    [switch]$Force,          # overwrite an existing .rustup / .cargo
    [switch]$WithVsInstall,  # run the VS installer from vs-layout (needs UAC)
    [switch]$AddToPath,      # append %USERPROFILE%\.cargo\bin to the user PATH
    [switch]$Check           # finish with cargo check in src-tauri
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjDir = Split-Path -Parent $ScriptDir
$RootDir = Split-Path -Parent $ProjDir

if ([string]::IsNullOrEmpty($BundleDir)) {
    $candidate = Join-Path $RootDir 'env-bundle'
    if (Test-Path -LiteralPath $candidate) { $BundleDir = $candidate }
}
if ([string]::IsNullOrEmpty($BundleDir) -or -not (Test-Path -LiteralPath $BundleDir)) {
    throw 'Bundle directory not found. Pass -BundleDir <path to env-bundle>.'
}
$BundleDir = (Get-Item -LiteralPath $BundleDir).FullName
$ManifestPath = Join-Path $BundleDir 'MANIFEST.txt'
if (-not (Test-Path -LiteralPath $ManifestPath)) { throw ('MANIFEST.txt missing in ' + $BundleDir) }

$RustupHome = Join-Path $env:USERPROFILE '.rustup'
$CargoHome  = Join-Path $env:USERPROFILE '.cargo'

function Step {
    param([string]$Text)
    Write-Host ''
    Write-Host ('--- ' + $Text) -ForegroundColor Yellow
}
function Ok   { param([string]$T) Write-Host ('    OK   ' + $T) -ForegroundColor Green }
function Warn { param([string]$T) Write-Host ('    WARN ' + $T) -ForegroundColor Yellow }
function Bad  { param([string]$T) Write-Host ('    FAIL ' + $T) -ForegroundColor Red }

function Copy-Tree {
    param([string]$Src, [string]$Dst, [string]$Label)
    if (-not (Test-Path -LiteralPath $Src)) { Warn ($Label + ' absent in bundle, skipped'); return $false }
    $parent = Split-Path -Parent $Dst
    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $rcArgs = @($Src, $Dst, '/E', '/MT:16', '/R:1', '/W:1', '/NFL', '/NDL', '/NJH', '/NJS', '/NP')
    & robocopy @rcArgs | Out-Null
    if ($LASTEXITCODE -ge 8) { throw ('robocopy failed for ' + $Label + ' (exit ' + $LASTEXITCODE + ')') }
    # -Property Length -Sum is required; without it Measure-Object returns no Sum
    # and every part prints "0 files, 0 MB".
    $m = Get-ChildItem -LiteralPath $Dst -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum
    $mb = 0
    if ($m.Sum) { $mb = [math]::Round($m.Sum / 1MB, 1) }
    Ok ($Label + '  ' + $m.Count + ' files, ' + $mb + ' MB')
    return $true
}

# ------------------------------------------------------------------- header
$manifestText = Get-Content -LiteralPath $ManifestPath -Raw
$recWorkspace = ''
$recRustc = ''
$recMsvc = ''
$recSdk = ''
$hasTarget = ''
foreach ($line in ($manifestText -split "`r?`n")) {
    if ($line -match '^workspace\s*:\s*(.+)$')  { $recWorkspace = $matches[1].Trim() }
    if ($line -match '^rustc\s*:\s*(.+)$')      { $recRustc = $matches[1].Trim() }
    if ($line -match '^msvc\s*:\s*(.+)$')       { $recMsvc = $matches[1].Trim() }
    if ($line -match '^winsdk\s*:\s*(.+)$')     { $recSdk = $matches[1].Trim() }
    if ($line -match '^has_target\s*:\s*(.+)$') { $hasTarget = $matches[1].Trim() }
}

# [cargo-bin-shims] section: one shim file name per line, until the next [section].
$shimList = New-Object System.Collections.Generic.List[string]
$inShims = $false
foreach ($line in ($manifestText -split "`r?`n")) {
    if ($line -match '^\s*\[cargo-bin-shims\]') { $inShims = $true; continue }
    if ($line -match '^\s*\[') { $inShims = $false; continue }
    if ($inShims) {
        $t = $line.Trim()
        if ($t -ne '') { [void]$shimList.Add($t) }
    }
}
# Fallback for a bundle produced before the shim list existed: rustup always
# installs exactly this set for a minimal-profile toolchain.
if ($shimList.Count -eq 0) {
    foreach ($n in @('cargo.exe','cargo-clippy.exe','cargo-fmt.exe','cargo-miri.exe','clippy-driver.exe','rls.exe','rust-analyzer.exe','rust-gdb.exe','rust-gdbgui.exe','rust-lldb.exe','rustc.exe','rustdoc.exe','rustfmt.exe')) {
        [void]$shimList.Add($n)
    }
}

Write-Host ''
Write-Host '=== ShotGlance environment bundle restore ===' -ForegroundColor Cyan
Write-Host ('    bundle    : ' + $BundleDir)
Write-Host ('    workspace : ' + $RootDir)
Write-Host ('    recorded  : ' + $recRustc)
Write-Host ('                msvc ' + $recMsvc + '   sdk ' + $recSdk)

# ------------------------------------------------------- 1. rust toolchain
Step '1/6 rust toolchain'

$tcSrc = Join-Path $BundleDir 'toolchain\rustup'
if (Test-Path -LiteralPath $tcSrc) {
    $busy = Get-Process -Name 'rustc','cargo','rustup' -ErrorAction SilentlyContinue
    if ($busy) { Warn 'rustc/cargo/rustup are running; close them if the copy fails' }

    if ((Test-Path -LiteralPath $RustupHome) -and -not $Force) {
        $existing = @(Get-ChildItem -LiteralPath (Join-Path $RustupHome 'toolchains') -Directory -ErrorAction SilentlyContinue)
        if ($existing.Count -gt 0) {
            Warn ('%USERPROFILE%\.rustup already has ' + $existing.Count + ' toolchain(s): ' + (($existing | Select-Object -ExpandProperty Name) -join ', '))
            Warn 're-run with -Force to overwrite'
        } else {
            Copy-Tree -Src $tcSrc -Dst $RustupHome -Label 'rustup-home' | Out-Null
        }
    } else {
        Copy-Tree -Src $tcSrc -Dst $RustupHome -Label 'rustup-home' | Out-Null
    }
} else {
    Warn 'toolchain\rustup not in bundle'
}

# The bundle ships only rustup.exe plus the shim NAMES, because on the source
# machine those 13 shims were symlinks to rustup.exe and copying them verbatim
# would mean 14 x 12.8 MB. Rebuild them as hard links: same volume, no admin
# rights needed, and rustup dispatches on argv[0] so a hard link behaves
# exactly like the original symlink.
$binSrc = Join-Path $BundleDir 'toolchain\cargo-bin'
$binDst = Join-Path $CargoHome 'bin'
if ((Test-Path -LiteralPath $binSrc) -and (Test-Path -LiteralPath (Join-Path $binSrc 'rustup.exe'))) {
    if (-not (Test-Path -LiteralPath $binDst)) { New-Item -ItemType Directory -Path $binDst -Force | Out-Null }
    $rustupDst = Join-Path $binDst 'rustup.exe'
    Copy-Item -LiteralPath (Join-Path $binSrc 'rustup.exe') -Destination $rustupDst -Force

    $linked = 0
    $copied = 0
    foreach ($s in $shimList) {
        $p = Join-Path $binDst $s
        if (Test-Path -LiteralPath $p) { $linked++; continue }
        try {
            New-Item -ItemType HardLink -Path $p -Target $rustupDst -ErrorAction Stop | Out-Null
            $linked++
        } catch {
            Copy-Item -LiteralPath $rustupDst -Destination $p -Force
            $copied++
        }
    }
    if ($copied -gt 0) {
        Ok ('cargo-bin  rustup.exe + ' + $linked + ' hard links + ' + $copied + ' real copies')
    } else {
        Ok ('cargo-bin  rustup.exe + ' + $linked + ' hard links')
    }
}

$regSrc = Join-Path $BundleDir 'toolchain\cargo-registry'
$regDst = Join-Path $CargoHome 'registry'
if (Test-Path -LiteralPath $regSrc) {
    foreach ($sub in @('cache', 'index')) {
        $s = Join-Path $regSrc $sub
        if (Test-Path -LiteralPath $s) {
            Copy-Tree -Src $s -Dst (Join-Path $regDst $sub) -Label ('cargo-registry/' + $sub) | Out-Null
        }
    }
} else {
    Warn 'toolchain\cargo-registry not in bundle - cargo will download every crate again'
}

$cfgSrc = Join-Path $BundleDir 'toolchain\cargo-home\config.toml'
if (Test-Path -LiteralPath $cfgSrc) {
    Copy-Item -LiteralPath $cfgSrc -Destination (Join-Path $CargoHome 'config.toml') -Force
    Ok 'cargo config.toml'
}

# ------------------------------------------------------------- 2. node_modules
Step '2/6 node_modules'
$nmSrc = Join-Path $BundleDir 'node_modules'
$nmDst = Join-Path $ProjDir 'node_modules'
if (Test-Path -LiteralPath $nmSrc) {
    Copy-Tree -Src $nmSrc -Dst $nmDst -Label 'node_modules' | Out-Null
    $tauri = Join-Path $nmDst '.bin\tauri.cmd'
    if (Test-Path -LiteralPath $tauri) { Ok 'tauri CLI present' } else { Warn 'node_modules\.bin\tauri.cmd missing - run npm ci' }
} else {
    Warn 'node_modules not in bundle - run npm ci (62 MB download)'
}

# ------------------------------------------------------------------ 3. target
Step '3/6 build artifacts'
$tgtSrc = Join-Path $BundleDir 'target'
$tgtDst = Join-Path $ProjDir 'src-tauri\target'
if (-not (Test-Path -LiteralPath $tgtSrc)) {
    Write-Host '    target not in bundle - first cargo check will compile everything (~30 min)'
} else {
    $samePath = $false
    if ($recWorkspace) {
        $a = $recWorkspace.TrimEnd('\')
        $b = $RootDir.TrimEnd('\')
        if ($a -ieq $b) { $samePath = $true }
    }
    if ($samePath) {
        Copy-Tree -Src $tgtSrc -Dst $tgtDst -Label 'target' | Out-Null
    } else {
        Warn 'workspace path differs from the recorded one:'
        Warn ('      recorded: ' + $recWorkspace)
        Warn ('      current : ' + $RootDir)
        Warn 'target skipped - Rust dep-info files embed absolute paths, so the'
        Warn 'cache would be treated as stale and rebuilt anyway.'
        Warn 'To reuse it, move the project to the recorded path and re-run.'
    }
}

# ------------------------------------------------------------- 4. MSVC / SDK
Step '4/6 MSVC + Windows SDK'
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$vsPath = ''
if (Test-Path -LiteralPath $vswhere) {
    $vsPath = (& $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>&1) -join ''
}
if ($vsPath) {
    $m = Get-ChildItem (Join-Path $vsPath 'VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
    $sdkRoot = 'C:\Program Files (x86)\Windows Kits\10\Include'
    $sdk = ''
    if (Test-Path -LiteralPath $sdkRoot) {
        $s = Get-ChildItem $sdkRoot -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
        if ($s) { $sdk = $s.Name }
    }
    Ok ('MSVC present: ' + $vsPath)
    if ($m) { Ok ('toolset ' + $m.Name) }
    if ($sdk) { Ok ('windows sdk ' + $sdk) } else { Warn 'no Windows SDK 10 headers found - build will fail' }
} else {
    Bad 'no MSVC C++ toolset on this machine'
    $layout = Join-Path $BundleDir 'vs-layout'
    $vsBoot = Join-Path $BundleDir 'installers\vs_BuildTools.exe'
    if ((Test-Path -LiteralPath $layout) -and (Test-Path -LiteralPath $vsBoot)) {
        Write-Host '    offline layout found - installing locally (no download)'
        if ($WithVsInstall) {
            # The 0x80070057 trap: the VS bootstrapper builds a case-insensitive
            # env dictionary, so having both HTTP_PROXY and http_proxy aborts it
            # before it even starts. Clear them in THIS process, then elevate:
            # a -Verb RunAs child inherits the caller's environment block.
            foreach ($n in @('http_proxy','https_proxy','all_proxy','no_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY')) {
                try { [Environment]::SetEnvironmentVariable($n, $null, 'Process') } catch { }
            }
            $layArgs = @(
                '--noWeb',
                '--add', 'Microsoft.VisualStudio.Workload.VCTools',
                '--add', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
                '--includeRecommended',
                '--quiet', '--wait', '--norestart'
            )
            Write-Host '    a UAC prompt will appear - click Yes'
            Start-Process -FilePath $vsBoot -ArgumentList $layArgs -Verb RunAs -Wait
            $vsPath2 = (& $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>&1) -join ''
            if ($vsPath2) { Ok ('MSVC installed: ' + $vsPath2) } else { Bad 'installer finished but no toolset found' }
        } else {
            $vsCmdLine = '& "' + $vsBoot + '" --noWeb --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --includeRecommended --quiet --wait --norestart'
            Write-Host ''
            Write-Host '    Run this in an ADMIN PowerShell (offline, ~10-15 min):' -ForegroundColor Cyan
            Write-Host ('      ' + $vsCmdLine)
            Write-Host '    Then re-run this script with -WithVsInstall, or just re-run it.'
            Write-Host '    If it exits with 0x80070057, clear proxy env vars first:'
            Write-Host '      foreach ($n in @("http_proxy","https_proxy","HTTP_PROXY","HTTPS_PROXY")) { [Environment]::SetEnvironmentVariable($n,$null,"Process") }'
        }
    } else {
        Warn 'no vs-layout in the bundle'
        Write-Host '    Install online (bootstrap exe is in installers\):'
        Write-Host ('      & "' + $vsBoot + '" --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --includeRecommended --quiet --wait --norestart')
        Write-Host '    That is the slow path - this is exactly what the bundle is meant to avoid.'
    }
}

# --------------------------------------------------------------- 5. PATH
Step '5/6 PATH'
$cargoBinDir = Join-Path $CargoHome 'bin'
$userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ([string]::IsNullOrEmpty($userPath)) { $userPath = '' }
$hasIt = $false
foreach ($seg in ($userPath -split ';')) {
    if ($seg.TrimEnd('\') -ieq $cargoBinDir.TrimEnd('\')) { $hasIt = $true }
}
if ($hasIt) {
    Ok ('user PATH already contains ' + $cargoBinDir)
} else {
    Warn ('user PATH does not contain ' + $cargoBinDir)
    if ($AddToPath) {
        $newPath = $cargoBinDir
        if ($userPath -ne '') { $newPath = $userPath.TrimEnd(';') + ';' + $cargoBinDir }
        [Environment]::SetEnvironmentVariable('PATH', $newPath, 'User')
        Ok 'appended to user PATH (open a new terminal to pick it up)'
    } else {
        # Do NOT suggest `setx PATH "%PATH%;..."`: %PATH% expands to user PATH
        # concatenated with the machine PATH, so setx would copy every system
        # entry into the user PATH and can also truncate at 1024 chars.
        Write-Host '    re-run with -AddToPath, or append it yourself with:'
        Write-Host ('      [Environment]::SetEnvironmentVariable("PATH", [Environment]::GetEnvironmentVariable("PATH","User") + ";" + "' + $cargoBinDir + '", "User")')
    }
}
Write-Host '    current session uses the restored binaries through absolute paths below'

# ---------------------------------------------------------------- 6. verify
Step '6/6 verify'
$rustcExe = Join-Path $cargoBinDir 'rustc.exe'
$cargoExe = Join-Path $cargoBinDir 'cargo.exe'
$allOk = $true
if (Test-Path -LiteralPath $rustcExe) {
    $v = (& $rustcExe -V 2>&1) -join ' '
    Ok $v
    if ($recRustc -and ($v -notlike ('*' + $recRustc + '*'))) {
        if ($v -notmatch [regex]::Escape(($recRustc -split ' ')[1])) {
            Warn ('version differs from the recorded one: ' + $recRustc)
        }
    }
} else {
    Bad 'rustc.exe not found - rust toolchain restore failed'
    $allOk = $false
}
if (Test-Path -LiteralPath $cargoExe) {
    Ok ((& $cargoExe -V 2>&1) -join ' ')
} else {
    Bad 'cargo.exe not found'
    $allOk = $false
}

if ($Check -and $allOk) {
    Write-Host ''
    Write-Host '    running cargo check (incremental if target was restored)...'
    $env:PATH = $cargoBinDir + ';' + $env:PATH
    Push-Location (Join-Path $ProjDir 'src-tauri')
    # cargo writes "Finished ..." to stderr. Under $ErrorActionPreference='Stop'
    # PowerShell turns that into a terminating NativeCommandError, so the script
    # died here even when the check itself succeeded (exit 0). Relax the
    # preference for the duration of the call and read $LASTEXITCODE instead.
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $cargoExe check 2>&1 | Select-Object -Last 15
        $checkCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prevEap
        Pop-Location
    }
    if ($checkCode -eq 0) { Ok 'cargo check passed' } else { Bad ('cargo check exit ' + $checkCode) }
}

Write-Host ''
if ($allOk) {
    Write-Host 'Done. Next:  powershell -ExecutionPolicy Bypass -File verify-env.ps1   (run from shotglance\)' -ForegroundColor Green
} else {
    Write-Host 'Finished with errors - see FAIL lines above.' -ForegroundColor Red
}
