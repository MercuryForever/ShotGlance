use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use serde::Serialize;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

mod capture;
mod ocr;

// 目录叫 translate，模块却叫 mt：lib.rs 里有个 `#[tauri::command] fn translate`，
// Rust 的路径解析碰到同名模块时会把 `translate::run` 当成函数调用，直接编译失败。
#[path = "translate/mod.rs"]
mod mt;

/// 面板窗口标签。capabilities/default.json 按这个值授权，改这里要同步改那里。
const PANEL: &str = "main";

/// 面板无活动多少秒后销毁。
///
/// 这是满足"空闲内存 < 80 MB"的关键：只要窗口活着，WebView2 就会常驻
/// 5–6 个子进程（实测约 292 MB）。回收窗口才是把内存降下来的唯一手段，
/// 压模型、调线程池都解决不了这个问题。
const IDLE_TIMEOUT_SECS: u64 = 120;

/// watchdog 扫描周期。粒度越细回收越及时，但唤醒次数越多。
const WATCHDOG_TICK_SECS: u64 = 5;

/// 最近一次活动时间（Unix 毫秒）。0 表示尚无活动。
static LAST_ACTIVE_MS: AtomicU64 = AtomicU64::new(0);

/// 选区阶段暂存的全屏帧。
///
/// 为什么要暂存：用户是把"看到的这一帧"框起来，识别也必须是这一帧。
/// 如果松手后重新抓屏，期间弹窗、页面滚动、鼠标悬停高亮都会让内容错位，
/// 而这种错位用户第一眼就能看出来。
///
/// 代价是整屏 RGBA 常驻 8–15 MB，但只在选区期间存在，松手或取消立刻释放。
static PENDING: Mutex<Option<PendingRegion>> = Mutex::new(None);

/// 进入选区前的窗口几何。
///
/// 面板被临时拉成全屏无边框，退出时必须回到用户原来摆放的位置和尺寸——
/// 回不到原位的话，每用一次框选窗口就跳一次，非常烦人。
static PANEL_GEOM: Mutex<Option<PanelGeom>> = Mutex::new(None);

struct PendingRegion {
    frame: image::RgbaImage,
    monitor: usize,
    capture_ms: u64,
}

#[derive(Clone, Copy)]
struct PanelGeom {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

/// 清掉选区暂存。面板销毁/取消/完成后都要走一遍，否则一帧整屏图像会一直跟着进程。
fn clear_pending() {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

fn is_selecting() -> bool {
    PENDING
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or(false)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 标记一次活动，推迟空闲回收。
fn touch() {
    LAST_ACTIVE_MS.store(now_ms(), Ordering::Relaxed);
}

fn idle_ms() -> u64 {
    let last = LAST_ACTIVE_MS.load(Ordering::Relaxed);
    if last == 0 {
        0
    } else {
        now_ms().saturating_sub(last)
    }
}

/// 实际使用的空闲阈值。允许用 SHOTGLANCE_IDLE_SECS 覆盖，
/// 便于自动化测试在十几秒内验证回收逻辑，而不必等满 120 秒。
fn idle_timeout_secs() -> u64 {
    std::env::var("SHOTGLANCE_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(IDLE_TIMEOUT_SECS)
}

/// 全局热键：Ctrl+Shift+S。
fn hotkey() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS)
}

/// 创建面板窗口；已存在则前置复用。
///
/// 复用分支很重要：窗口刚建好 120 秒内再次触发热键，走的是 show+focus
/// （约 5 ms），而不是重建 WebView2（约 300–500 ms）。
fn open_panel(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(PANEL) {
        let _ = w.unminimize();
        w.show()?;
        w.set_focus()?;
        touch();
        return Ok(w);
    }

    // SHOTGLANCE_AUTO_OCR=1 时前端加载完就跑一次识别，用于自动化取界面证据
    // （无法模拟点击时的替代方案）。
    let url = if std::env::var_os("SHOTGLANCE_AUTO_OCR").is_some() {
        WebviewUrl::App("index.html?auto=ocr".into())
    } else {
        WebviewUrl::App("index.html".into())
    };

    let mut builder = WebviewWindowBuilder::new(app, PANEL, url)
        .title("ShotGlance")
        .inner_size(520.0, 460.0)
        .min_inner_size(360.0, 260.0)
        .resizable(true)
        .center();

    // 置顶开关：截屏自检时窗口会被桌面上的其它应用盖住，导致"看起来没打开"。
    // 也顺便作为后续"译文悬浮层"的预留位。
    if std::env::var_os("SHOTGLANCE_TOPMOST").is_some() {
        builder = builder.always_on_top(true);
    }

    let w = builder.build()?;

    w.set_focus()?;
    touch();

    // 建窗的同时后台预热采集链路：首次 capture 要 273 ms（初始化 D3D11/WGC），
    // 预热后降到 39 ms。窗口出现本身要几百毫秒，正好把这个成本藏进去。
    std::thread::spawn(capture::warm_up_once);

    Ok(w)
}

/// 销毁面板窗口。WebView2 子进程随之全部退出。
fn destroy_panel(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PANEL) {
        let _ = w.destroy();
    }
    // 选区状态跟着窗口一起作废：留着的话下一轮打开面板会看到一个
    // 早已过期的裁剪结果。
    clear_pending();
    PANEL_GEOM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    // 面板没了就把 OCR 引擎一起还掉。ONNX Runtime 的会话内存（权重 + arena）
    // 有几十 MB 且不会自己缩，留着就等于把"空闲 < 80 MB"这条红线让出去。
    // 下次识别重新加载约 250 ms——用户按下热键到看见窗口本来就要这么久，
    // 感知上被完全吸收。
    ocr::unload();
    // 翻译模型同理，而且它更大（113 MB 权重，常驻 150 MB 量级）。
    // 面板一关就还回去，否则"空闲 < 80 MB"立刻破功。
    mt::offline::unload();
}

/// 把面板窗口临时变成"整屏选区面"：无边框、置顶、铺满目标显示器。
///
/// 复用同一个窗口而不是新开一个，理由和内存红线同源：多一个 WebView2 窗口
/// 就多 5–6 个子进程（实测约 290 MB）。选区是瞬时交互，不值得为此付这份内存。
///
/// 注意这里用的是 Tauri 的显示器几何（物理像素），与 xcap 的帧尺寸同一套坐标系；
/// 万一两者不一致，前端还有一层"按实际渲染尺寸换算"的兜底（见 main.ts）。
fn enter_selection_mode(app: &AppHandle, monitor: usize) {
    let Some(win) = app.get_webview_window(PANEL) else {
        return;
    };

    // 先记几何再改窗口，顺序反了就会把"全屏尺寸"记成原始位置。
    if let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) {
        *PANEL_GEOM
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(PanelGeom { x: pos.x, y: pos.y, w: size.width, h: size.height });
    }

    // 只取几何值，不持有 Monitor：省的 API 差异（是否 Clone）跟编译器纠缠。
    let bounds = app
        .available_monitors()
        .ok()
        .and_then(|list| {
            if list.is_empty() {
                None
            } else {
                let m = &list[monitor.min(list.len() - 1)];
                Some((*m.position(), *m.size()))
            }
        })
        .or_else(|| {
            app.primary_monitor()
                .ok()
                .flatten()
                .map(|m| (*m.position(), *m.size()))
        });

    // 顺序有讲究：先摘掉标题栏再去设尺寸。
    // 带着标题栏时 set_size 设的是客户区，摘掉之后客户区会变大一圈，
    // 结果就是选区底图比屏幕多出一条，框出来的坐标整体偏移。
    //
    // 这里刻意不调 set_resizable(false)：Windows 上它会给窗口写一条
    // min/max track size，随后的 set_size 会被 WM_GETMINMAXINFO 夹回原尺寸，
    // 表现就是"量到 2560×1440，屏幕上还是 520×460"。
    // 全屏之后用户也没有边缘可拖，禁用缩放本来就没意义。
    let _ = win.set_decorations(false);
    let _ = win.set_always_on_top(true);

    if let Some((pos, size)) = bounds {
        let _ = win.set_size(tauri::PhysicalSize::new(size.width, size.height));

        // 外框比客户区大几个像素：Windows 会保留一圈不可见的缩放边框，
        // 即使 set_decorations(false) 也还在（实测 2560×1440 的客户区对应
        // 2576×1449 的外框）。按外框原点摆窗口，画面会整体右下偏 8×4 像素，
        // 用户框到的位置和看到的屏幕就对不上。用 inner_position 反算偏移补回来。
        let offset = match (win.outer_position(), win.inner_position()) {
            (Ok(outer), Ok(inner)) => (inner.x - outer.x, inner.y - outer.y),
            _ => (0, 0),
        };
        let _ = win.set_position(tauri::PhysicalPosition::new(
            pos.x - offset.0,
            pos.y - offset.1,
        ));

        // 只留一行日志：选区面的实际尺寸是"框不准"类问题的唯一取证线索，
        // 而这些数字全都来自运行时（显示器几何 / DWM 边框），没法靠读代码推断。
        eprintln!(
            "[shotglance] 选区面 {}x{}+{},{} (offset {:?})",
            size.width, size.height, pos.x, pos.y, offset
        );
    } else {
        eprintln!("[shotglance] 选区面拿不到显示器几何，窗口保持原尺寸");
    }

    let _ = win.set_focus();
}

/// 还原面板窗口的常规形态与原始几何。取消、完成、失败都要走这里。
fn exit_selection_mode(app: &AppHandle) {
    let Some(win) = app.get_webview_window(PANEL) else {
        return;
    };

    // 置顶要回到用户原本的设置，而不是一律关掉：
    // SHOTGLANCE_TOPMOST=1 是它原本就被要求置顶的显式开关。
    let _ = win.set_always_on_top(std::env::var_os("SHOTGLANCE_TOPMOST").is_some());
    // 还原同理：先装回标题栏，再按客户区尺寸还原，最后摆回原位。
    let _ = win.set_decorations(true);

    let geom = PANEL_GEOM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(g) = geom {
        let _ = win.set_size(tauri::PhysicalSize::new(g.w, g.h));
        let _ = win.set_position(tauri::PhysicalPosition::new(g.x, g.y));
    }

    let _ = win.set_focus();
}

fn toggle_panel(app: &AppHandle) {
    if app.get_webview_window(PANEL).is_some() {
        destroy_panel(app);
    } else {
        let _ = open_panel(app);
    }
}

/// 空闲回收线程。
///
/// 用 std::thread 而不是 async 任务：不需要引入 tokio 依赖，
/// 而且 AppHandle 是 Send + Sync，跨线程调用是安全的（内部派发到事件循环）。
fn spawn_idle_watchdog(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(WATCHDOG_TICK_SECS));
        // 选区进行中不回收：用户可能正盯着屏幕挑区域，此时把窗口抽走
        // 会让选区面消失但 PENDING 帧还在，状态就乱了。
        if app.get_webview_window(PANEL).is_some()
            && !is_selecting()
            && idle_ms() > idle_timeout_secs() * 1000
        {
            destroy_panel(&app);
        }
    });
}

/// 采集指定显示器整屏，返回 PNG(base64) 与分段时间。
///
/// 走 spawn_blocking：截屏是同步阻塞调用（等 DXGI 帧 20–60 ms），
/// 直接放在 async 里会占住 tokio worker，热键连按时容易堆积。
#[tauri::command]
async fn capture_screen(monitor: Option<usize>) -> Result<capture::CaptureResult, String> {
    touch();
    tauri::async_runtime::spawn_blocking(move || capture::capture_monitor(monitor))
        .await
        .map_err(|e| format!("采集任务异常: {e}"))?
}

/// 枚举显示器（多屏选择用）。
#[tauri::command]
fn list_monitors() -> Result<Vec<capture::MonitorInfo>, String> {
    capture::list_monitors()
}

/// 一次"截图 + 识别"的完整结果。
///
/// 预览图和 OCR 用的是**同一帧**：分两次采集会差几十毫秒，若期间页面滚动，
/// 出现的框就会和看到的图对不上——这类错位用户第一时间就会发现。
#[derive(Serialize)]
struct OcrScreen {
    monitor: usize,
    width: u32,
    height: u32,
    capture_ms: u64,
    encode_ms: u64,
    ocr: ocr::OcrResult,
    jpeg_b64: String,
}

/// 截图并识别。
///
/// 走 spawn_blocking：整条链路是同步 CPU 密集（采集 + 推理），
/// 放到 tokio worker 上会把别的 IPC 一起堵住。
#[tauri::command]
async fn ocr_screen(monitor: Option<usize>) -> Result<OcrScreen, String> {
    touch();
    tauri::async_runtime::spawn_blocking(move || {
        use std::time::Instant;
        let (frame, capture_ms, idx) = capture::grab_frame(monitor)?;
        let (width, height) = (frame.width(), frame.height());

        let ocr = ocr::recognize(&frame)?;

        let t = Instant::now();
        let jpeg_b64 = capture::encode_preview(&frame, 85)?;
        let encode_ms = t.elapsed().as_millis() as u64;

        Ok(OcrScreen {
            monitor: idx,
            width,
            height,
            capture_ms,
            encode_ms,
            ocr,
            jpeg_b64,
        })
    })
    .await
    .map_err(|e| format!("识别任务异常: {e}"))?
}

/// 区域框选的第一步返回：整屏帧（当遮罩底图）+ 尺寸。
///
/// 底图不是装饰：用户需要看着真实画面决定框哪儿，而不是对着黑屏盲框。
/// 顺便它也替代了"实时取景"——冻结画面比实时画面更适合框选，
/// 因为内容不会在拖动过程中滚动。
#[derive(Serialize)]
struct RegionBegin {
    monitor: usize,
    width: u32,
    height: u32,
    capture_ms: u64,
    jpeg_b64: String,
}

/// 区域框选完成后的结果。
#[derive(Serialize)]
struct RegionResult {
    monitor: usize,
    /// 选区在整屏帧里的原点（物理像素），用于界面显示"取自屏幕哪里"。
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    capture_ms: u64,
    crop_ms: u64,
    encode_ms: u64,
    jpeg_b64: String,
    /// run_ocr=false 时为 null（只要截图不要识别）。
    ocr: Option<ocr::OcrResult>,
}

/// 进入区域框选：抓一帧 → 暂存 → 把面板拉成整屏选区面。
///
/// 先抓屏再改窗口形态，顺序不能反：窗口一铺满，屏幕上出现的就是自己，
/// 此时再抓屏只会抓到遮罩。
#[tauri::command]
async fn region_begin(app: AppHandle, monitor: Option<usize>) -> Result<RegionBegin, String> {
    touch();
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (frame, capture_ms, idx) = capture::grab_frame(monitor)?;
        let (width, height) = (frame.width(), frame.height());

        // 底图用 q85：与预览一致，用户看到的边界就是他框出来的边界。
        let jpeg_b64 = capture::encode_preview(&frame, 85)?;

        {
            let mut slot = PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            *slot = Some(PendingRegion { frame, monitor: idx, capture_ms });
        }

        enter_selection_mode(&handle, idx);
        touch();

        Ok(RegionBegin { monitor: idx, width, height, capture_ms, jpeg_b64 })
    })
    .await
    .map_err(|e| format!("选区任务异常: {e}"))?
}

/// 完成框选：还原窗口 → 从暂存帧裁剪 → 可选 OCR → 返回预览与结果。
///
/// 窗口还原放在最前面是有意的：用户松手后界面必须立刻回到面板形态，
/// 剩下的裁剪与识别在后台跑。顺序反过来的话，全屏遮罩会一直挂着直到
/// OCR 结束，观感上像卡死。
#[tauri::command]
async fn region_finish(
    app: AppHandle,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    run_ocr: Option<bool>,
) -> Result<RegionResult, String> {
    touch();
    exit_selection_mode(&app);

    let pending = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .ok_or("没有待处理的选区画面（可能已取消）")?;

    let want_ocr = run_ocr.unwrap_or(true);

    tauri::async_runtime::spawn_blocking(move || {
        use std::time::Instant;

        let t = Instant::now();
        let cropped = capture::crop(&pending.frame, x, y, w, h)?;
        let crop_ms = t.elapsed().as_millis() as u64;

        let frame = cropped.image;
        let (width, height) = (frame.width(), frame.height());

        // 识别只吃裁剪后的图：这是全流程提速的全部来源——
        // 全屏 811 ms 里有 699 ms 花在逐行识别，框一小块就只剩几行。
        let ocr = if want_ocr { Some(ocr::recognize(&frame)?) } else { None };

        let t = Instant::now();
        let jpeg_b64 = capture::encode_preview(&frame, 85)?;
        let encode_ms = t.elapsed().as_millis() as u64;

        Ok(RegionResult {
            monitor: pending.monitor,
            x: cropped.x,
            y: cropped.y,
            width,
            height,
            capture_ms: pending.capture_ms,
            crop_ms,
            encode_ms,
            jpeg_b64,
            ocr,
        })
    })
    .await
    .map_err(|e| format!("选区任务异常: {e}"))?
}

/// 取消框选：还原窗口并丢掉暂存帧。
///
/// 同步命令（没有 await 的必要）：Esc 之后窗口要马上恢复，
/// 走异步只会让遮罩多挂一帧。
#[tauri::command]
fn region_cancel(app: AppHandle) {
    exit_selection_mode(&app);
    clear_pending();
}

/// OCR 依赖是否就位（模型 + onnxruntime.dll）。
/// 前端用它决定要不要禁用「识别」按钮，并直接把原因显示出来。
#[tauri::command]
fn ocr_ready() -> Result<(), String> {
    ocr::preflight()
}

/// 翻译当前识别出的文本。用设置里保存的引擎/配置。
///
/// 走 `spawn_blocking`：HTTP 请求和 ONNX 推理都是阻塞的，
/// 直接在 async 命令里做会把 Tauri 的异步 worker 卡住。
#[tauri::command]
async fn translate(app: AppHandle, text: String) -> Result<mt::Outcome, String> {
    touch();
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || mt::run(&a, &text))
        .await
        .map_err(|e| format!("翻译任务异常：{e}"))?
}

/// 引擎清单 + 当前配置下的可用状态，给设置面板渲染下拉框。
#[tauri::command]
fn translate_engines(app: AppHandle) -> Vec<mt::EngineInfo> {
    mt::engines(&mt::load_cfg(&app))
}

#[tauri::command]
fn translate_cfg_get(app: AppHandle) -> mt::Cfg {
    mt::load_cfg(&app)
}

#[tauri::command]
fn translate_cfg_set(app: AppHandle, cfg: mt::Cfg) -> Result<(), String> {
    mt::save_cfg(&app, &cfg)
}

/// 用指定引擎试译一句固定文本，让用户在设置里当场验证密钥/网络是否可用。
#[tauri::command]
async fn translate_test(app: AppHandle, engine: String) -> Result<mt::Outcome, String> {
    touch();
    let mut cfg = mt::load_cfg(&app);
    cfg.engine = engine;
    // 试译固定短句：内容稳定，用户能一眼看出翻得对不对
    let probe = "Save changes before closing?";
    tauri::async_runtime::spawn_blocking(move || mt::run_with(&cfg, probe))
        .await
        .map_err(|e| format!("试译任务异常：{e}"))?
}

/// 离线模型状态（是否已下载、占用多大、放在哪）。
#[tauri::command]
fn mt_status(app: AppHandle) -> mt::offline::ModelStatus {
    // 顺带把随包词表铺到模型目录，让"手动放两个 onnx"这条路径也能直接跑。
    let _ = mt::offline::ensure_vocab(&app);
    mt::offline::status()
}

/// 下载离线模型（113 MB）。进度通过 `mt-progress` 事件推送。
#[tauri::command]
async fn mt_download(app: AppHandle) -> Result<(), String> {
    touch();
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || mt::offline::download(&a))
        .await
        .map_err(|e| format!("下载任务异常：{e}"))?
}

/// 打开模型目录，方便用户自己往里放文件。
#[tauri::command]
fn mt_open_dir(app: AppHandle) -> Result<(), String> {
    let dir = mt::offline::models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建目录失败：{e}"))?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("打开目录失败：{e}"))?;
    let _ = &app;
    Ok(())
}

/// 关闭面板（前端 Esc 调用）。不是退出应用，只是回收窗口。
#[tauri::command]
fn close_panel(app: AppHandle) {
    destroy_panel(&app);
}

/// 解析 `x,y,w,h`（物理像素）。格式不对就返回 None，自检退化成整屏。
fn parse_crop_spec(spec: &str) -> Option<(f64, f64, f64, f64)> {
    let p: Vec<f64> = spec
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();
    if p.len() == 4 {
        Some((p[0], p[1], p[2], p[3]))
    } else {
        None
    }
}

/// 无界面翻译自检：把每个引擎都试一句，结论落盘后退出。
///
/// 在线引擎的"通不通"取决于网络和密钥，离线引擎的"对不对"取决于模型文件，
/// 这两件事都没法靠肉眼看界面来断言。自检把它们变成可脚本化的输出。
fn mt_self_test(app: &AppHandle) -> std::path::PathBuf {
    use std::fmt::Write as _;
    use std::time::Instant;

    let cfg = mt::load_cfg(app);
    let mut out = String::new();
    let _ = writeln!(out, "engine={} source={} target={}", cfg.engine, cfg.source, cfg.target);

    let st = mt::offline::status();
    let _ = writeln!(
        out,
        "models_installed={} bytes={} dir={}",
        st.installed, st.bytes, st.dir
    );

    let probe = std::env::var("SHOTGLANCE_MT_TEXT").unwrap_or_else(|_| {
        "Failed to connect to the server. Please check your network settings.".to_string()
    });
    let _ = writeln!(out, "probe={probe}");

    // 先跑一次 auto：这是用户实际会走的那条路，它决定了"按下翻译到看见
    // 译文"要等多久。放在逐引擎测试之前，避免被前面产生的冷却状态干扰。
    {
        let mut c = cfg.clone();
        c.engine = "auto".into();
        let t = Instant::now();
        match mt::run_with(&c, &probe) {
            Ok(r) => {
                let _ = writeln!(
                    out,
                    "[OK ] {:<26} {:5} ms  total={} ms  {}",
                    format!("auto → {}", r.engine),
                    r.ms,
                    t.elapsed().as_millis(),
                    r.text.replace('\n', " ⏎ ")
                );
                if let Some(n) = r.note {
                    let _ = writeln!(out, "      note: {n}");
                }
            }
            Err(e) => {
                let _ = writeln!(
                    out,
                    "[ERR] {:<26} {:5} ms  {}",
                    "auto",
                    t.elapsed().as_millis(),
                    e.replace('\n', " | ")
                );
            }
        }
        let _ = writeln!(out, "---");
    }

    // 第二轮用另一句文本（避开缓存），验证失败熔断确实生效：
    // google 在上一轮超时了，这一轮应该被直接跳过，总耗时应明显下降。
    {
        let mut c = cfg.clone();
        c.engine = "auto".into();
        let t = Instant::now();
        match mt::run_with(&c, "Save changes before closing?") {
            Ok(r) => {
                let _ = writeln!(
                    out,
                    "[OK ] {:<26} {:5} ms  total={} ms  {}",
                    format!("auto(第2次) → {}", r.engine),
                    r.ms,
                    t.elapsed().as_millis(),
                    r.text.replace('\n', " ⏎ ")
                );
            }
            Err(e) => {
                let _ = writeln!(out, "[ERR] auto(第2次) {} {}", t.elapsed().as_millis(), e);
            }
        }
        let _ = writeln!(out, "---");
    }

    for info in mt::engines(&cfg) {
        let mut c = cfg.clone();
        c.engine = info.id.clone();
        let t = Instant::now();
        // 试译绕过缓存：缓存命中会让"这个引擎到底通不通"失去意义
        match mt::run_with(&c, &probe) {
            Ok(r) => {
                let _ = writeln!(
                    out,
                    "[OK ] {:<26} {:5} ms  {}",
                    info.id, r.ms, r.text.replace('\n', " ⏎ ")
                );
            }
            Err(e) => {
                let _ = writeln!(
                    out,
                    "[ERR] {:<26} {:5} ms  {}",
                    info.id,
                    t.elapsed().as_millis(),
                    e.replace('\n', " | ")
                );
            }
        }
    }

    let path = std::env::temp_dir().join("shotglance-mt-selftest.txt");
    let _ = std::fs::write(&path, out);
    path
}

/// 无界面 OCR 自检：采集 → 识别 → 结果落盘，然后退出。
///
/// 识别正确率与时延都必须在真机上量，而"打开窗口点按钮"没法进 CI。
/// 把结论写成文本文件，脚本就能断言（行数 > 0、total_ms < 预算）。
fn ocr_self_test() -> std::path::PathBuf {
    use std::time::Instant;

    let path = std::env::temp_dir().join("shotglance-ocr-selftest.txt");
    let mut out = String::new();

    let t = Instant::now();
    match ocr::engine() {
        Ok(guard) => {
            let load_ms = guard.as_ref().map(|e| e.load_ms).unwrap_or(0);
            out.push_str(&format!(
                "engine_ok load_ms={load_ms} first_call_ms={}\n",
                t.elapsed().as_millis()
            ));
        }
        Err(e) => {
            out.push_str(&format!("engine_err={e}\n"));
            let _ = std::fs::write(&path, out);
            return path;
        }
    }

    // 指定图片时不抓屏。让"准确率"可复现：屏幕内容随时会变，
    // 同一张图跑出的行数与文本才能被脚本断言。
    let frame_result = match std::env::var_os("SHOTGLANCE_OCR_IMG") {
        Some(p) => match image::open(&p) {
            Ok(img) => Ok((img.to_rgba8(), 0u64, usize::MAX)),
            Err(e) => Err(format!("读取测试图失败: {e}")),
        },
        None => capture::grab_frame(None),
    };

    match frame_result {
        Ok((frame, cap_ms, idx)) => {
            out.push_str(&format!(
                "capture_ms={cap_ms} monitor={idx} size={}x{}\n",
                frame.width(),
                frame.height()
            ));

            // SHOTGLANCE_CROP=x,y,w,h：把选区裁剪也跑一遍。
            // 裁剪是纯计算，越界、取整、贴边这些边界只有用固定坐标才验得准。
            let frame = match std::env::var("SHOTGLANCE_CROP")
                .ok()
                .and_then(|v| parse_crop_spec(&v))
            {
                Some((cx, cy, cw, ch)) => match capture::crop(&frame, cx, cy, cw, ch) {
                    Ok(c) => {
                        out.push_str(&format!(
                            "crop_req={cx},{cy},{cw},{ch} crop_at={},{} crop_size={}x{}\n",
                            c.x,
                            c.y,
                            c.image.width(),
                            c.image.height()
                        ));
                        c.image
                    }
                    Err(e) => {
                        out.push_str(&format!("crop_err={e}\n"));
                        frame
                    }
                },
                None => frame,
            };

            match ocr::recognize(&frame) {
                Ok(r) => {
                    out.push_str(&format!(
                        "det_ms={} rec_ms={} total_ms={} det_size={}x{} lines={}\n",
                        r.det_ms,
                        r.rec_ms,
                        r.total_ms,
                        r.det_size[0],
                        r.det_size[1],
                        r.lines.len()
                    ));
                    for l in r.lines.iter().take(40) {
                        out.push_str(&format!(
                            "[{},{} {}x{}] {:.2} {}\n",
                            l.x, l.y, l.w, l.h, l.conf, l.text
                        ));
                    }
                    out.push_str("\n--- text ---\n");
                    out.push_str(&r.text);
                }
                Err(e) => out.push_str(&format!("ocr_err={e}\n")),
            }
        }
        Err(e) => out.push_str(&format!("capture_err={e}\n")),
    }

    let _ = std::fs::write(&path, out);

    // 保持进程存活，供外部脚本采样内存。
    // SHOTGLANCE_UNLOAD_TEST=1 时先卸载再保持，用来验证"卸载后能回到基线"。
    let hold = std::env::var("SHOTGLANCE_HOLD_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if hold > 0 {
        if std::env::var_os("SHOTGLANCE_UNLOAD_TEST").is_some() {
            ocr::unload();
        }
        std::thread::sleep(std::time::Duration::from_secs(hold));
    }

    path
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if shortcut == &hotkey() && event.state() == ShortcutState::Pressed {
                        let _ = open_panel(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            // 随包词表铺到模型目录。应用内下载和"用户自己拷两个 onnx 进来"
            // 两条路都依赖它先就位。
            let _ = mt::offline::ensure_vocab(app.handle());

            // 无界面翻译自检：逐引擎试一句，落盘后退出。
            if std::env::var_os("SHOTGLANCE_MT_SELFTEST").is_some() {
                let path = mt_self_test(app.handle());
                println!("mt self-test written to {}", path.display());
                std::process::exit(0);
            }

            // 无界面 OCR 自检（优先级最高：它是纯计算，不需要托盘/热键）。
            if std::env::var_os("SHOTGLANCE_OCR_SELFTEST").is_some() {
                let path = ocr_self_test();
                println!("ocr self-test written to {}", path.display());
                std::process::exit(0);
            }

            // 无界面自检：SHOTGLANCE_SELF_TEST=1 时采集一次、落盘、退出。
            // 让截图链路可被脚本验证（COM / DPI / 多屏只有在真机跑才暴露问题）。
            if std::env::var_os("SHOTGLANCE_SELF_TEST").is_some() {
                let path = capture::self_test();
                println!("self-test written to {}", path.display());
                std::process::exit(0);
            }

            // 热键注册失败不应导致启动失败：Linux/Wayland 下 GlobalShortcuts
            // portal 多数 compositor 未实现，此时降级为托盘菜单触发。
            if let Err(e) = app.global_shortcut().register(hotkey()) {
                eprintln!("[shotglance] 全局热键注册失败，改用托盘触发: {e}");
            }

            let toggle = MenuItem::with_id(app, "toggle", "显示 / 隐藏面板", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle, &quit])?;

            TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .tooltip("ShotGlance — Ctrl+Shift+S 打开面板")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "toggle" => toggle_panel(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_panel(tray.app_handle());
                    }
                })
                .build(app)?;

            spawn_idle_watchdog(app.handle().clone());

            // 可测性开关：设置 SHOTGLANCE_OPEN_PANEL=1 时启动即建面板。
            // 用途是让内存/冷启动测试能脚本化（含 CI），不改变默认行为。
            if std::env::var_os("SHOTGLANCE_OPEN_PANEL").is_some() {
                // 建窗失败必须打出来：静默忽略会让"界面打不开"变成一个
                // 完全无迹可查的空白，只能靠猜。
                match open_panel(app.handle()) {
                    Ok(w) => eprintln!(
                        "[shotglance] 面板已建: visible={:?} pos={:?} size={:?}",
                        w.is_visible(),
                        w.outer_position(),
                        w.inner_size()
                    ),
                    Err(e) => eprintln!("[shotglance] 建窗失败: {e}"),
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != PANEL {
                return;
            }
            if let tauri::WindowEvent::Focused(true) = event {
                touch();
            }
        })
        .invoke_handler(tauri::generate_handler![
            capture_screen,
            list_monitors,
            ocr_screen,
            region_begin,
            region_finish,
            region_cancel,
            ocr_ready,
            translate,
            translate_engines,
            translate_cfg_get,
            translate_cfg_set,
            translate_test,
            mt_status,
            mt_download,
            mt_open_dir,
            close_panel
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building ShotGlance");

    app.run(|_app, event| {
        // 面板销毁后应用必须继续驻留托盘。
        // code 为 None 表示"最后一个窗口关闭"引发的退出请求——拦掉；
        // code 为 Some(_) 才是 app.exit(code) 的主动退出，放行。
        if let RunEvent::ExitRequested { code, api, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}
