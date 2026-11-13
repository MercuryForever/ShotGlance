<#
    restore-nsis-cache.ps1
    ----------------------
    Prepares the NSIS toolchain that Tauri needs, WITHOUT requiring GitHub access.

    Why this exists:
      `cargo tauri build` downloads nsis-3.13.zip and nsis_tauri_utils.dll from
      github.com. In networks where github.com is blocked (CONNECT 502), the
      bundling step fails. This script populates the cache from local vendor
      files, or via a GitHub mirror.

    Result layout (must match Tauri's expectation):
      %LOCALAPPDATA%\tauri\NSIS\
        Bin\makensis.exe
        Stubs\lzma-x86-unicode
        Stubs\lzma_solid-x86-unicode
        Plugins\x86-unicode\additional\nsis_tauri_utils.dll   <-- required
        Plugins\x86-unicode\nsis_tauri_utils.dll              <-- makensis load path

    Usage:
      powershell -ExecutionPolicy Bypass -File .\scripts\restore-nsis-cache.ps1
      powershell -ExecutionPolicy Bypass -File .\scripts\restore-nsis-cache.ps1 -Force
#>
param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$Ns  = 'nsis-3.13'
$Tgt = Join-Path $env:LOCALAPPDATA 'tauri\NSIS'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Vendor = Join-Path $Here 'vendor'

$ZipName = 'nsis-3.13.zip'
$DllName = 'nsis_tauri_utils.dll'
$ZipSha  = 'BA63DFFC4410EE89193E1CB5A41989991BD77C61068DA17E3156D136B7B0B3D8'
$DllSha  = '5BA143B5DB4A87D32D6E7802E033330AAE56CBCEABE0D1E3BA41948385AD4709'

$ZipUrl = "https://ghproxy.net/https://github.com/tauri-apps/binary-releases/releases/download/$Ns/$ZipName"
$DllUrl = 'https://ghproxy.net/https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll'

function Test-Cache {
    $required = @(
        'Bin\makensis.exe',
        'Stubs\lzma-x86-unicode',
        'Stubs\lzma_solid-x86-unicode',
        'Plugins\x86-unicode\additional\nsis_tauri_utils.dll'
    )
    foreach ($r in $required) {
        if (-not (Test-Path (Join-Path $Tgt $r))) { return $false }
    }
    return $true
}

function Get-Asset {
    param([string]$Name, [string]$Url, [string]$ExpectedSha)

    $local = Join-Path $Vendor $Name
    $tmp   = Join-Path $env:TEMP $Name

    if (Test-Path $local) {
        Write-Host "  [local] $Name"
        $path = $local
    } else {
        Write-Host "  [mirror] downloading $Name ..."
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing -TimeoutSec 300
        $path = $tmp
    }

    $sha = (Get-FileHash -Path $path -Algorithm SHA256).Hash
    if ($sha -ne $ExpectedSha) {
        throw "SHA256 mismatch for $Name`n  expected $ExpectedSha`n  actual   $sha"
    }
    Write-Host "  [ok] sha256 verified"

    # Normalize into vendor/ so future runs are offline
    if (-not (Test-Path $Vendor)) { New-Item -ItemType Directory -Path $Vendor | Out-Null }
    if (-not (Test-Path $local)) { Copy-Item $path $local -Force }
    return $path
}

Write-Host "Tauri NSIS cache -> $Tgt"

if ((Test-Path $Tgt) -and (Test-Cache) -and -not $Force) {
    Write-Host "Already complete. Nothing to do. (use -Force to rebuild)"
    exit 0
}

Write-Host ""
Write-Host "Step 1/4  acquiring assets"
$zipPath = Get-Asset -Name $ZipName -Url $ZipUrl -ExpectedSha $ZipSha
$dllPath = Get-Asset -Name $DllName -Url $DllUrl -ExpectedSha $DllSha

Write-Host "Step 2/4  staging"
$stage = Join-Path $env:TEMP ('nsis-stage-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $stage | Out-Null
Expand-Archive -Path $zipPath -DestinationPath $stage -Force

$extracted = Join-Path $stage $Ns
if (-not (Test-Path $extracted)) { throw "Unexpected archive layout: $extracted not found" }

Write-Host "Step 3/4  installing to cache"
if (Test-Path $Tgt) { Remove-Item $Tgt -Recurse -Force }
New-Item -ItemType Directory -Path $Tgt | Out-Null
Get-ChildItem -Path $extracted -Force | Copy-Item -Destination $Tgt -Recurse -Force

# Plugin must exist in BOTH locations:
#   additional\  -> what tauri-bundler validates
#   x86-unicode\ -> what makensis actually loads at compile time
$plugDir = Join-Path $Tgt 'Plugins\x86-unicode'
$addDir  = Join-Path $plugDir 'additional'
if (-not (Test-Path $addDir)) { New-Item -ItemType Directory -Path $addDir | Out-Null }
Copy-Item $dllPath (Join-Path $addDir $DllName) -Force
Copy-Item $dllPath (Join-Path $plugDir $DllName) -Force

Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "Step 4/4  verifying"
if (Test-Cache) {
    $n = (Get-ChildItem $Tgt -Recurse -File).Count
    $mb = [math]::Round(((Get-ChildItem $Tgt -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 1)
    Write-Host "  PASS  $n files, $mb MB"
    Write-Host ""
    Write-Host "Done. 'npm run tauri:build' will no longer touch the network for NSIS."
    exit 0
} else {
    Write-Host "  FAIL  cache still incomplete"
    exit 1
}
