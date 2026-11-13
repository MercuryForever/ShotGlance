//! 屏幕采集。
//!
//! 选型说明：用 `xcap` 而不是自己写三套原生调用。
//! - Windows 下 `wgc` feature 走 Windows.Graphics.Capture（DXGI 桌面复制之上），
//!   比 GDI BitBlt 更适合多屏 / HDR / DPI 缩放场景。
//! - 代价是多引入 `windows` crate（约几百 KB）与一层间接调用；
//!   换来的是 macOS(ScreenCaptureKit) / Linux(X11,Wayland) 同一套 API，
//!   省掉三套平台分支和各自的回归成本。包体预算 150 MB，这里花得起。

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use base64::Engine as _;
use serde::Serialize;
use xcap::Monitor;

/// 采集链路是否已预热。进程内只需一次。
static WARMED: AtomicBool = AtomicBool::new(false);

/// 显示器摘要。给设置页/多屏选择用。
#[derive(Serialize, Clone, Debug)]
pub struct MonitorInfo {
    pub index: usize,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
}

/// 一次采集的结果。
///
/// 时延拆成 capture / encode 两段，因为两者的优化手段完全不同：
/// capture 是采集链路（DXGI 帧等待），encode 是压缩（纯 CPU）。
/// 混在一起看不出瓶颈在哪。
///
/// 实测（2560×1440）：采集稳态 39–45 ms，但首次 273 ms；编码 PNG 134 ms、
/// JPEG q85 83 ms。所以瓶颈从不是采集，而是首次初始化与编码器选择。
#[derive(Serialize)]
pub struct CaptureResult {
    pub monitor: usize,
    pub width: u32,
    pub height: u32,
    pub capture_ms: u64,
    pub encode_ms: u64,
    pub total_ms: u64,
    /// JPEG 预览图（base64）。用 JPEG 而非 PNG：体积 1.93 MB → 553 KB，
    /// 编码 134 ms → 83 ms。预览是给人看的，无损没有意义；
    /// OCR 走内存里的原始 RGBA，不经过这条路径，所以不影响识别精度。
    pub jpeg_b64: String,
}

pub fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    let monitors = Monitor::all().map_err(|e| format!("枚举显示器失败: {e}"))?;

    Ok(monitors
        .iter()
        .enumerate()
        .map(|(index, m)| MonitorInfo {
            index,
            name: m.name().unwrap_or_else(|_| format!("显示器 {index}")),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            scale: m.scale_factor().unwrap_or(1.0),
            primary: m.is_primary().unwrap_or(false),
        })
        .collect())
}

/// 抓一帧原始 RGBA（不编码）。
///
/// 单独抽出来是因为 OCR 链路要的就是原始像素：走 JPEG 编码再解码会白花
/// 80 ms 编码 + 40 ms 解码，还会因为 4:2:0 色度下采样丢掉小字笔画。
/// 预览才需要 JPEG，识别不需要。
pub fn grab_frame(index: Option<usize>) -> Result<(image::RgbaImage, u64, usize), String> {
    let started = Instant::now();

    let monitors = Monitor::all().map_err(|e| format!("枚举显示器失败: {e}"))?;
    if monitors.is_empty() {
        return Err("没有检测到可用显示器".into());
    }

    // None 时优先主显示器，而不是死取第 0 个——多屏下两者经常不是同一块。
    let idx = match index {
        Some(i) => i.min(monitors.len() - 1),
        None => monitors
            .iter()
            .position(|m| m.is_primary().unwrap_or(false))
            .unwrap_or(0),
    };

    let frame = monitors[idx]
        .capture_image()
        .map_err(|e| format!("采集失败: {e}"))?;

    Ok((frame, started.elapsed().as_millis() as u64, idx))
}

/// 裁剪结果：子图 + 它在整屏帧里的原点。
///
/// 原点要带出来，因为前端要把"选区左上角"换算成"屏幕坐标"显示给用户，
/// 只回一张图会让这个换算在前端再算一遍。
pub struct Cropped {
    pub image: image::RgbaImage,
    pub x: u32,
    pub y: u32,
}

/// 从整屏帧里裁一块（坐标是物理像素，相对帧左上角）。
///
/// 采集链路完全不用改：区域截图就是全屏帧的一次子区域拷贝。
/// 取整 + 夹取放在这里而不是调用方——越界坐标在"拖动出屏"、多屏 DPI 不一致、
/// 以及前端换算误差下都会出现，让每个调用点各写一遍迟早会漏。
pub fn crop(frame: &image::RgbaImage, x: f64, y: f64, w: f64, h: f64) -> Result<Cropped, String> {
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
        return Err("选区坐标无效".into());
    }

    let fw = frame.width().max(1) as f64;
    let fh = frame.height().max(1) as f64;

    let x0 = x.round().clamp(0.0, fw - 1.0) as u32;
    let y0 = y.round().clamp(0.0, fh - 1.0) as u32;
    let cw = w.round().clamp(1.0, fw - x0 as f64) as u32;
    let ch = h.round().clamp(1.0, fh - y0 as f64) as u32;

    let image = image::imageops::crop_imm(frame, x0, y0, cw, ch).to_image();
    Ok(Cropped { image, x: x0, y: y0 })
}

/// 采集指定显示器整屏（None = 主显示器）。
///
/// 整屏采集与区域采集的区别只在最后有没有一次裁剪，
/// 采集本身（枚举 + WGC 取帧）是同一段代码。
pub fn capture_monitor(index: Option<usize>) -> Result<CaptureResult, String> {
    let started = Instant::now();
    let (frame, capture_ms, idx) = grab_frame(index)?;

    let (width, height) = (frame.width(), frame.height());

    // 编码单独计时。RGB 转换也算进去——那同样是用户要等的时间。
    let enc = Instant::now();
    let jpeg_b64 = encode_preview(&frame, 85)?;
    let encode_ms = enc.elapsed().as_millis() as u64;

    Ok(CaptureResult {
        monitor: idx,
        width,
        height,
        capture_ms,
        encode_ms,
        total_ms: started.elapsed().as_millis() as u64,
        jpeg_b64,
    })
}

/// RGBA → JPEG(base64)。预览与 OCR 共用同一个编码器参数，避免"预览看着清楚、
/// 识别却不一致"这种排查起来极费劲的偏差。
pub fn encode_preview(frame: &image::RgbaImage, quality: u8) -> Result<String, String> {
    let rgb = image::DynamicImage::ImageRgba8(frame.clone()).to_rgb8();
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    image::DynamicImage::ImageRgb8(rgb)
        .write_with_encoder(encoder)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// 预热采集链路。
///
/// 首次 `capture_image()` 要初始化 D3D11 设备与 WGC 帧池，实测 273 ms，
/// 是稳态（39 ms）的 7 倍。这段开销不该落在用户第一次按热键的路径上，
/// 所以在建窗时后台跑一次、丢弃结果——用户感知到的就只剩稳态耗时。
///
/// 代价（实测，且不可逆）：xcap 用 `LazyLock` 全局单例缓存
/// `ID3D11Device` / `IDXGIDevice` / `GraphicsCaptureItem`，没有 clear API。
/// 所以预热后常驻内存从 23 MB 升到 65 MB，且进程退出前不会还回去。
/// 好消息是它**只初始化一次**，不会随截图次数累积，不是泄漏。
///
/// 设 `SHOTGLANCE_WARMUP=0` 可关闭：换来 23 MB 常驻，代价是首次采集 351 ms。
/// 两种取舍都留有硬指标余量，让用户按机器情况自己选。
pub fn warm_up_once() {
    if std::env::var("SHOTGLANCE_WARMUP").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    if WARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Ok(monitors) = Monitor::all() {
        if let Some(m) = monitors.first() {
            let _ = m.capture_image();
        }
    }
}

/// 无界面自检：枚举显示器 + 采集多次，结果写入临时文件。
///
/// 存在的理由：截图链路最容易出问题的地方是 COM 初始化、DPI 缩放、
/// 多屏坐标，而这些只有在真机上跑才会暴露。有了这个入口，
/// 验证就不依赖"打开窗口手动点按钮"。
pub fn self_test() -> std::path::PathBuf {
    let path = std::env::temp_dir().join("shotglance-selftest.txt");
    let mut out = String::new();

    match list_monitors() {
        Ok(monitors) => {
            out.push_str(&format!("monitors={}\n", monitors.len()));
            for m in monitors {
                out.push_str(&format!(
                    "monitor index={} {}x{} scale={} primary={} name={}\n",
                    m.index, m.width, m.height, m.scale, m.primary, m.name
                ));
            }
        }
        Err(e) => out.push_str(&format!("list_err={e}\n")),
    }

    // 冷启动基线（不做预热），用来量化预热到底省了多少。
    match capture_monitor(None) {
        Ok(r) => {
            out.push_str(&format!(
                "cold_total_ms={} capture_ms={} encode_ms={} jpeg_bytes={}\n",
                r.total_ms,
                r.capture_ms,
                r.encode_ms,
                r.jpeg_b64.len() * 3 / 4
            ));
            // 落盘一张图：排障时"我看到的界面"比任何文字描述都准。
            if let Some(p) = std::env::var_os("SHOTGLANCE_DUMP_IMG") {
                use base64::Engine as _;
                if let Ok(bytes) =
                    base64::engine::general_purpose::STANDARD.decode(&r.jpeg_b64)
                {
                    let _ = std::fs::write(&p, bytes);
                }
            }
        }
        Err(e) => out.push_str(&format!("cold_err={e}\n")),
    }

    // 模拟真实路径：建窗时预热，之后再连采三轮。
    // 这三轮才代表用户按热键时实际看到的时延。
    warm_up_once();
    for round in 1..=3 {
        match capture_monitor(None) {
            Ok(r) => out.push_str(&format!(
                "warm_round{} {}x{} capture_ms={} encode_ms={} total_ms={} jpeg_bytes={}\n",
                round,
                r.width,
                r.height,
                r.capture_ms,
                r.encode_ms,
                r.total_ms,
                r.jpeg_b64.len() * 3 / 4
            )),
            Err(e) => out.push_str(&format!("warm_round{}_err={e}\n", round)),
        }
    }

    // 编码器对比：同一帧分别走 PNG / JPEG，决定预览用哪个。
    if let Ok(monitors) = Monitor::all() {
        if let Some(m) = monitors.first() {
            if let Ok(frame) = m.capture_image() {
                let (png_ms, png_len) = bench_png(&frame);
                let (jpg_ms, jpg_len) = bench_jpeg(&frame, 85);
                out.push_str(&format!("encode_png ms={png_ms} bytes={png_len}\n"));
                out.push_str(&format!("encode_jpeg_q85 ms={jpg_ms} bytes={jpg_len}\n"));
            }
        }
    }

    let _ = std::fs::write(&path, out);
    path
}

/// 编码基准：PNG。拷贝放到计时之外，只量压缩本身。
fn bench_png(img: &image::RgbaImage) -> (u64, usize) {
    let owned = image::DynamicImage::ImageRgba8(img.clone());
    let t = Instant::now();
    let mut cursor = Cursor::new(Vec::new());
    let _ = owned.write_to(&mut cursor, image::ImageFormat::Png);
    let ms = t.elapsed().as_millis() as u64;
    (ms, cursor.into_inner().len())
}

/// 编码基准：JPEG。先转 RGB8（JPEG 无 alpha），转换同样放在计时之外。
fn bench_jpeg(img: &image::RgbaImage, quality: u8) -> (u64, usize) {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let owned = image::DynamicImage::ImageRgb8(rgb);
    let t = Instant::now();
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    let _ = owned.write_with_encoder(encoder);
    let ms = t.elapsed().as_millis() as u64;
    (ms, buf.len())
}
