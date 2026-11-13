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
    Hint      = $Hint
  })
}

Write-Host ""
Write-Host "=== 1. 工具链 ===" -ForegroundColor Cyan

$rustc = Get-Command rustc -ErrorAction SilentlyContinue
if ($rustc) {
  $rv = (& rustc -V) -join ' '
  Add-Row 'Rust' $rv $true ''
} else {
  Add-Row 'Rust' '未安装 (rustup 缺失)' $false '运行 rustup-init.exe -y --default-toolchain stable-msvc --profile minimal'
}

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($cargo) {
  $cv = (& cargo -V) -join ' '
  Add-Row 'Cargo' $cv $true ''
} else {
  Add-Row 'Cargo' '未安装' $false '随 rustup 一起安装'
}

$tauriCli = Get-Command tauri -ErrorAction SilentlyContinue
if ($tauriCli) {
  Add-Row 'Tauri CLI' ((& tauri -V) -join ' ') $true ''
} else {
  $pkg = Join-Path (Get-Location) 'node_modules\.bin\tauri.cmd'
  if (Test-Path $pkg) {
    Add-Row 'Tauri CLI' 'node_modules 内可用' $true ''
  } else {
    Add-Row 'Tauri CLI' '未安装' $false 'npm i -D @tauri-apps/cli'
  }
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
  Add-Row 'Python(模型导出)' "$pv (需 3.10/3.11)" $okPy 'paddlepaddle 不支持 3.13，需单独建 3.11 venv'
} else {
  Add-Row 'Python(模型导出)' '未安装' $false '仅模型导出需要，可后置'
}

Write-Host "=== 2. 系统级依赖 ===" -ForegroundColor Cyan

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$msvcPath = $null
if (Test-Path $vswhere) {
  $msvcPath = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath) -join ''
}
if ($msvcPath) {
  Add-Row 'MSVC Build Tools' $msvcPath $true ''
} else {
  Add-Row 'MSVC Build Tools' '未安装 —— 唯一硬阻塞项' $false '需管理员: winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools"'
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
