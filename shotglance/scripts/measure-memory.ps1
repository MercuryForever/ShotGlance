<#
    measure-memory.ps1
    ------------------
    Proves the on-demand window lifecycle actually reclaims memory.

    Background:
      A live WebView2 window keeps ~5-6 msedgewebview2.exe child processes alive.
      Measured on this machine that is ~292 MB on top of a ~24 MB Rust process.
      That blows the "< 80 MB idle" budget. The only fix is DESTROYING the
      window when idle, not hiding it - hidden windows keep child processes alive.

    Three stages:
      A  resident   -> app started with SHOTGLANCE_NO_PANEL=1, no panel.
                       Expect 0 WebView2 children.
      B  panel open -> panel created (now the DEFAULT startup behaviour).
                       Expect many.
      C  reclaimed  -> after idle timeout. Expect back to stage A levels.
                       The startup panel is exempt from reclamation by default,
                       so stage B/C also set SHOTGLANCE_RECLAIM_STARTUP=1.

    Usage:
      powershell -ExecutionPolicy Bypass -File .\scripts\measure-memory.ps1
      powershell -ExecutionPolicy Bypass -File .\scripts\measure-memory.ps1 -IdleSecs 20
      # -LogPath also writes every line to a file (useful in CI / non-interactive runs)
#>
param(
    [int]$IdleSecs = 10,
    [int]$BudgetMb = 80,
    [string]$LogPath = '',
    # Disable capture warm-up: resident memory drops to ~23 MB,
    # but the first capture costs ~351 ms. Used to compare the two trade-offs.
    # NOTE: keep this file ASCII-only. PowerShell 5.1 reads BOM-less files as ANSI,
    # so non-ASCII comments here corrupt parsing.
    [switch]$NoWarmup
)

$ErrorActionPreference = 'Stop'

if ($LogPath) { Set-Content -Path $LogPath -Value '' -Encoding utf8 }

function Say {
    param([string]$Message)
    Write-Output $Message
    if ($LogPath) { Add-Content -Path $LogPath -Value $Message -Encoding utf8 }
}

# 构建区在仓库根 build\target（见 src-tauri/.cargo/config.toml）
$exe = Join-Path $PSScriptRoot '..\..\build\target\release\shotglance.exe'
if (-not (Test-Path $exe)) { throw "executable not found: $exe (run npm run tauri:build first)" }
$exe = (Resolve-Path $exe).Path

function Get-Wv2Ids {
    @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
}

function Stop-App {
    Get-Process -Name shotglance -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    for ($i = 0; $i -lt 10; $i++) {
        if (-not (Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue)) { break }
        Start-Sleep -Milliseconds 300
    }
}

function Measure-Stage {
    param([int]$ProcId, [int[]]$WvBaseline, [string]$Name)

    $main = Get-Process -Id $ProcId -ErrorAction SilentlyContinue
    if (-not $main) { throw "process $ProcId exited unexpectedly during stage '$Name'" }
    $main.Refresh()

    $newIds = @(Get-Wv2Ids | Where-Object { $WvBaseline -notcontains $_ })
    $children = @()
    if ($newIds.Count -gt 0) {
        $children = @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue |
                      Where-Object { $newIds -contains $_.Id })
    }

    $mainMb = [math]::Round($main.WorkingSet64 / 1MB, 1)
    $wvMb = 0
    if ($children.Count -gt 0) {
        $wvMb = [math]::Round((($children | Measure-Object WorkingSet64 -Sum).Sum) / 1MB, 1)
    }

    [pscustomobject]@{
        Stage   = $Name
        MainMb  = $mainMb
        WvCount = $children.Count
        WvMb    = $wvMb
        TotalMb = [math]::Round($mainMb + $wvMb, 1)
        Pid     = $ProcId
    }
}

Say "ShotGlance memory profile"
Say "  exe        : $exe"
Say "  idle secs  : $IdleSecs"
Say "  budget     : $BudgetMb MB"
if ($NoWarmup) { Say "  warmup     : OFF (expect ~23 MB resident, first capture slower)" }
else { Say "  warmup     : ON  (expect ~65 MB resident, first capture fast)" }
Say ""

Stop-App
$results = @()

# ---------- Stage A: resident, no window ----------
Say "[A] resident (tray only, no panel) ..."
$wvBase = Get-Wv2Ids
# App shows its main window at startup since v0.2; pin it to tray-only for this stage.
$env:SHOTGLANCE_NO_PANEL = '1'
$pA = Start-Process -FilePath $exe -PassThru
Remove-Item Env:SHOTGLANCE_NO_PANEL -ErrorAction SilentlyContinue
Start-Sleep -Seconds 4
$results += Measure-Stage -ProcId $pA.Id -WvBaseline $wvBase -Name 'A resident'

# ---------- Stage B: panel open ----------
Say "[B] panel open (cold start measured to first window) ..."
Stop-App
$wvBase = Get-Wv2Ids

$env:SHOTGLANCE_RECLAIM_STARTUP = '1'
$env:SHOTGLANCE_IDLE_SECS = "$IdleSecs"
if ($NoWarmup) { $env:SHOTGLANCE_WARMUP = '0' }
$sw = [Diagnostics.Stopwatch]::StartNew()
$pB = Start-Process -FilePath $exe -PassThru
Remove-Item Env:SHOTGLANCE_RECLAIM_STARTUP -ErrorAction SilentlyContinue
Remove-Item Env:SHOTGLANCE_IDLE_SECS -ErrorAction SilentlyContinue
Remove-Item Env:SHOTGLANCE_WARMUP -ErrorAction SilentlyContinue

$coldMs = -1
while ($sw.ElapsedMilliseconds -lt 20000) {
    $pB.Refresh()
    if ($pB.HasExited) { break }
    if ($pB.MainWindowHandle -ne 0) { $coldMs = $sw.ElapsedMilliseconds; break }
    Start-Sleep -Milliseconds 25
}
$sw.Stop()
Start-Sleep -Seconds 3
$results += Measure-Stage -ProcId $pB.Id -WvBaseline $wvBase -Name 'B panel open'

# ---------- Stage C: reclaimed after idle ----------
# Poll instead of sleeping a fixed amount: WebView2 children take 4-8 s to exit
# after the window is destroyed, so a fixed wait can sample mid-teardown and
# report a false failure.
Say "[C] waiting for idle reclamation (idle=${IdleSecs}s, poll limit $($IdleSecs + 25)s) ..."
$poll = 0
$limit = $IdleSecs + 25
while ($poll -lt $limit) {
    Start-Sleep -Seconds 2
    $poll += 2
    $pending = @(Get-Wv2Ids | Where-Object { $wvBase -notcontains $_ })
    if ($pending.Count -eq 0) { break }
}
Say "    reclamation observed after ~${poll}s"
Start-Sleep -Seconds 1
$results += Measure-Stage -ProcId $pB.Id -WvBaseline $wvBase -Name 'C reclaimed'

Stop-App

# ---------- report ----------
Say ""
$fmt = "{0,-14} {1,9} {2,9} {3,9} {4,9}"
Say ($fmt -f 'stage', 'main MB', 'wv procs', 'wv MB', 'total MB')
Say ($fmt -f ('-' * 14), ('-' * 9), ('-' * 9), ('-' * 9), ('-' * 9))
foreach ($r in $results) {
    Say ($fmt -f $r.Stage, $r.MainMb, $r.WvCount, $r.WvMb, $r.TotalMb)
}

$a = $results[0]; $b = $results[1]; $c = $results[2]
$fail = 0

Say ""
if ($a.TotalMb -lt $BudgetMb) {
    Say ("PASS  resident {0} MB < {1} MB budget" -f $a.TotalMb, $BudgetMb)
} else {
    Say ("FAIL  resident {0} MB exceeds {1} MB" -f $a.TotalMb, $BudgetMb); $fail++
}

if ($a.WvCount -eq 0) {
    Say "PASS  no WebView2 child process while resident"
} else {
    Say ("FAIL  {0} WebView2 children alive at startup" -f $a.WvCount); $fail++
}

if ($b.WvCount -gt 0) {
    Say ("INFO  panel open costs {0} MB across {1} WebView2 children (why reclamation is required)" -f $b.TotalMb, $b.WvCount)
} else {
    Say "FAIL  panel never opened; stage B is not meaningful"; $fail++
}

if ($c.WvCount -eq 0 -and $c.TotalMb -lt $BudgetMb) {
    Say ("PASS  reclaimed to {0} MB after idle (0 WebView2 children)" -f $c.TotalMb)
} else {
    Say ("FAIL  not reclaimed: {0} MB, {1} children" -f $c.TotalMb, $c.WvCount); $fail++
}

if ($coldMs -ge 0) {
    Say ("INFO  cold start to first window: {0} ms" -f $coldMs)
} else {
    Say "WARN  could not observe window handle; cold start unmeasured"
}

Say ""
Say "failures: $fail"
exit $fail
