param(
  [string]$Out = "tests\ocr-sample.png"
)

# Generates a deterministic OCR test image with GDI+ text rendering.
# ASCII only: PowerShell 5.1 reads BOM-less UTF-8 as ANSI, so non-ASCII
# comments here would break the param block.

Add-Type -AssemblyName System.Drawing

$w = 1000
$h = 560

$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

$brush = [System.Drawing.Brushes]::Black
$gray = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(90, 90, 90))

$lines = @(
  @{ T = "截图翻译工具 ShotGlance";                  S = 34; X = 40; Y = 30;  B = $brush },
  @{ T = "轻量级桌面 OCR：检测与识别全离线运行";      S = 24; X = 40; Y = 100; B = $brush },
  @{ T = "内存占用低于 80 MB，冷启动 2 秒内";         S = 18; X = 40; Y = 160; B = $brush },
  @{ T = "Hello world - OCR 2026-11-09 11:20";      S = 16; X = 40; Y = 215; B = $brush },
  @{ T = "这是一行 14 像素的小字，用来测试检测阈值";    S = 14; X = 40; Y = 270; B = $gray },
  @{ T = "The quick brown fox jumps over 0123456789"; S = 13; X = 40; Y = 320; B = $gray },
  @{ T = "第二段：标点符号，。！？；：「引号」";        S = 20; X = 40; Y = 380; B = $brush },
  @{ T = "Mixed 中英混排 line with numbers 3.14159";  S = 17; X = 40; Y = 440; B = $brush }
)

foreach ($l in $lines) {
  $font = New-Object System.Drawing.Font(
    "Microsoft YaHei",
    [float]$l.S,
    [System.Drawing.FontStyle]::Regular,
    [System.Drawing.GraphicsUnit]::Pixel
  )
  $g.DrawString($l.T, $font, $l.B, [float]$l.X, [float]$l.Y)
  $font.Dispose()
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }

$full = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out))
$bmp.Save($full, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()

Write-Output "wrote $full"
