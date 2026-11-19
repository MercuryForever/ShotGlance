<#
  verify-env.ps1 — 截图 / OCR / 翻译工具链环境验收脚本
  目标技术栈: Rust + Tauri 2 + ONNX Runtime(ort) + PP-OCRv4 + Vite/TS

  用法:
    powershell -ExecutionPolicy Bypass -File .\verify-env.ps1
    powershell -ExecutionPolicy Bypass -File .\verify-env.ps1 -Full   # 额外跑 cargo check

  判定标准: 所有 required 项均为 PASS，才可以开始 cargo tauri build
#>
param([switch]$Full)

$ErrorActionPreference = 'SilentlyContinue'
$rows = New-Object System.Collections.ArrayList

function Add-Row {
  param([string]$Name, [string]$Detail, [bool]$Ok, [string]$Hint, [bool]$Required = $true)
  [void]$rows.Add([pscustomobject]@{
    Component = $Name
    Status    = $(if ($Ok) { 'PASS' } else { $(if ($Required) { 'FAIL' } else { 'WARN' }) })
    Detail    = $Detail
    # PASS 行不显示 Hint，否则表格里「已通过」旁边还挂着「去修一下」很容易被误读
    Hint      = $(if ($Ok) { '' } else { $Hint })
  })
}

Write-Host ""
Write-Host "=== 1. 工具链 ===" -ForegroundColor Cyan

# 刚跑完 rustup-init 的会话里 PATH 还是旧的，所以要回退到默认安装位置探测
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$rustcOnPath = (Get-Command rustc -ErrorAction SilentlyContinue).Source
$rustcExe = $rustcOnPath
if (-not $rustcExe -and (Test-Path (Join-Path $cargoBin 'rustc.exe'))) { $rustcExe = Join-Path $cargoBin 'rustc.exe' }
$cargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargoExe -and (Test-Path (Join-Path $cargoBin 'cargo.exe'))) { $cargoExe = Join-Path $cargoBin 'cargo.exe' }
$pathNote = $(if ($rustcExe -and -not $rustcOnPath) { '  [PATH 未生效，重开终端或把 .cargo\bin 加入 PATH]' } else { '' })

if ($rustcExe) {
  Add-Row 'Rust' (((& $rustcExe -V) -join ' ') + $pathNote) $true ''
} else {
  Add-Row 'Rust' '未安装 (rustup 缺失)' $false '运行 rustup-init.exe -y --default-toolchain 1.99.1-x86_64-pc-windows-msvc --profile minimal（锁旧版本才能复用 build/target）'
}

if ($cargoExe) {
  Add-Row 'Cargo' (((& $cargoExe -V) -join ' ') + $pathNote) $true ''
} else {
  Add-Row 'Cargo' '未安装' $false '随 rustup 一起安装'
}

$tauriCli = Get-Command tauri -ErrorAction SilentlyContinue
# node_modules 可能不在当前目录（本仓库在 shotglance/ 子目录下），逐层找
$tauriLocal = $null
foreach ($b in @((Get-Location).Path, $PSScriptRoot, (Join-Path (Get-Location).Path 'shotglance'), (Join-Path $PSScriptRoot 'shotglance'))) {
  if (-not $b) { continue }
  $c = Join-Path $b 'node_modules\.bin\tauri.cmd'
  if (Test-Path $c) { $tauriLocal = $c; break }
}
if ($tauriCli) {
  Add-Row 'Tauri CLI' ((& tauri -V) -join ' ') $true ''
} elseif ($tauriLocal) {
  Add-Row 'Tauri CLI' ((& $tauriLocal -V) -join ' ') $true ''
} else {
  Add-Row 'Tauri CLI' '未安装' $false 'npm i -D @tauri-apps/cli'
}

$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) {
  $nv = (& node -v)
  $major = [int]($nv.TrimStart('v').Split('.')[0])
  $minor = [int]($nv.TrimStart('v').Split('.')[1])
  $okNode = ($major -gt 20) -or ($major -eq 20 -and $minor -ge 19)
  Add-Row 'Node.js' "$nv (需求 >= 20.19)" $okNode '升级 Node 到 20.19+ 或 22 LTS'
} else {
  Add-Row 'Node.js' '未安装' $false '安装 Node 22 LTS'
}

$py = Get-Command python -ErrorAction SilentlyContinue
if ($py) {
  $pv = (& python -V) -join ' '
  $okPy = $pv -match '3\.(10|11|12)'
  Add-Row 'Python(模型导出)' "$pv (需 3.10/3.11，仅重导模型用)" $okPy 'paddlepaddle 不支持 3.13，需单独建 3.11 venv' $false
} else {
  Add-Row 'Python(模型导出)' '未安装' $false '仅重导模型需要，不影响编译，可后置' $false
}

Write-Host "=== 2. 系统级依赖 ===" -ForegroundColor Cyan

# 找 MSVC: 以磁盘上真实的 cl.exe 为准，而不是以 VS 产品是否注册为准。
# 两个必须注意的点:
#   1) vswhere 必须带 -all -prerelease，否则 VS 预览版（如 Build Tools 2026）根本列不出来
#   2) 装了 VS 不等于装了 C++ 工具集：残留目录可能只有 Auxiliary + lib，没有 bin/include
function Find-Msvc {
  $roots = @()
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (Test-Path $vswhere) {
    $roots += (& $vswhere -all -prerelease -products * -property installationPath)
  }
  # 兜底: vswhere 缺失或实例未注册时，直接扫安装根的两层目录
  foreach ($base in @((Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio'), (Join-Path $env:ProgramFiles 'Microsoft Visual Studio'))) {
    if (-not (Test-Path $base)) { continue }
    foreach ($prod in (Get-ChildItem $base -Directory)) {
      foreach ($inst in (Get-ChildItem $prod.FullName -Directory)) { $roots += $inst.FullName }
    }
  }
  $found = @()
  foreach ($r in ($roots | Where-Object { $_ } | Select-Object -Unique)) {
    $toolsRoot = Join-Path $r 'VC\Tools\MSVC'
    if (-not (Test-Path $toolsRoot)) { continue }
    foreach ($t in (Get-ChildItem $toolsRoot -Directory)) {
      $cl = Join-Path $t.FullName 'bin\Hostx64\x64\cl.exe'
      if (Test-Path $cl) { $found += [pscustomobject]@{ Root = $r; Toolset = $t.Name; Cl = $cl } }
    }
  }
  if ($found.Count -eq 0) { return $null }
  return ($found | Sort-Object Toolset -Descending | Select-Object -First 1)
}

# 找 Windows SDK: 光有 MSVC 不够，缺 Lib\um\x64\kernel32.lib 就链接不了
function Find-Sdk {
  foreach ($r in @((Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'), (Join-Path $env:ProgramFiles 'Windows Kits\10'))) {
    $lib = Join-Path $r 'Lib'
    if (-not (Test-Path $lib)) { continue }
    $v = Get-ChildItem $lib -Directory | Where-Object {
      (Test-Path (Join-Path $_.FullName 'um\x64\kernel32.lib')) -and
      (Test-Path (Join-Path $_.FullName 'ucrt\x64\ucrt.lib'))
    } | Sort-Object Name -Descending | Select-Object -First 1
    if ($v) {
      $hdr = Test-Path (Join-Path $r ('Include\{0}\um\windows.h' -f $v.Name))
      return [pscustomobject]@{ Version = $v.Name; Root = $r; HasHeader = $hdr }
    }
  }
  return $null
}

$msvc = Find-Msvc
if ($msvc) {
  Add-Row 'MSVC Build Tools' ('{0}  [{1}]' -f $msvc.Toolset, $msvc.Root) $true ''
} else {
  Add-Row 'MSVC Build Tools' '未找到 cl.exe（只有空壳目录也按未装处理）' $false 'VS Installer 勾选「使用 C++ 的桌面开发」；或 winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools"'
}

$sdk = Find-Sdk
if ($sdk) {
  Add-Row 'Windows SDK' ('{0}  [{1}]' -f $sdk.Version, $sdk.Root) $sdk.HasHeader 'SDK 缺 Include\um\windows.h，请在 VS Installer 里修复'
} else {
  Add-Row 'Windows SDK' '未找到 um\x64\kernel32.lib —— 无法链接' $false 'VS Installer 勾选 Windows 11 SDK；或单独安装 Windows SDK 10.0.26100'
}

$wvKeys = @(
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
  'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
  'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
)
$wv = $null
foreach ($k in $wvKeys) {
  $p = Get-ItemProperty $k -ErrorAction SilentlyContinue
  if ($p -and $p.pv) { $wv = $p.pv; break }
}
if (-not $wv) {
  $wvDir = 'C:\Program Files (x86)\Microsoft\EdgeWebView\Application'
  if (Test-Path $wvDir) {
    $wv = ((Get-ChildItem $wvDir -Directory | Where-Object { $_.Name -match '^\d+\.' } | Sort-Object Name -Descending | Select-Object -First 1).Name) + ' (推断自目录)'
  }
}
if ($wv) {
  Add-Row 'WebView2 Runtime' $wv $true ''
} else {
  Add-Row 'WebView2 Runtime' '未检测到' $false 'Win11 自带；否则装 Evergreen Bootstrapper'
}

$cm = Get-Command cmake -ErrorAction SilentlyContinue
if ($cm) {
  Add-Row 'CMake' ((& cmake --version | Select-Object -First 1) -join '') $true '' $false
} else {
  Add-Row 'CMake' '未安装 (仅精简 ONNX 构建需要)' $false '先走 ort 预编译二进制，体积超标再补' $false
}

Write-Host "=== 3. 主机资源 ===" -ForegroundColor Cyan
$cpu = (Get-CimInstance Win32_Processor).NumberOfLogicalProcessors
$ram = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
$free = [math]::Round((Get-PSDrive C).Free / 1GB, 1)
Write-Host ("  逻辑核数 : {0}" -f $cpu)
Write-Host ("  物理内存 : {0} GB" -f $ram)
Write-Host ("  C 盘剩余 : {0} GB  (构建环境需 11-16 GB)" -f $free)
if ($free -lt 16) { Write-Host "  [!] 磁盘余量不足以完成首次全量构建" -ForegroundColor Yellow }

Write-Host ""
Write-Host "=== 验收结果 ===" -ForegroundColor Cyan
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

$fail = @($rows | Where-Object { $_.Status -eq 'FAIL' }).Count
$warn = @($rows | Where-Object { $_.Status -eq 'WARN' }).Count

if ($fail -eq 0) {
  Write-Host ("环境就绪 (0 FAIL, {0} WARN)，可以执行: cargo tauri build" -f $warn) -ForegroundColor Green
} else {
  Write-Host ("存在 {0} 项 FAIL，按 Hint 列处理后重跑本脚本" -f $fail) -ForegroundColor Red
}

if ($Full -and $fail -eq 0) {
  Write-Host ""
  Write-Host "=== 附加: cargo check ===" -ForegroundColor Cyan
  if (Test-Path 'src-tauri\Cargo.toml') {
    Push-Location 'src-tauri'
    & cargo check --message-format short
    $code = $LASTEXITCODE
    Pop-Location
    Write-Host ("cargo check 退出码: {0}" -f $code)
  } else {
    Write-Host "未找到 src-tauri\Cargo.toml，跳过"
  }
}

exit $fail
