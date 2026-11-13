//! OCR 引擎：PP-OCRv4（DB 检测 + CRNN 识别），ONNX Runtime 单后端。
//!
//! 为什么不走 PaddleOCR 的 C++ 推理库：包体会从 20 MB 涨到 300 MB 以上，
//! 且要随包携带整套 Paddle 依赖。ONNX Runtime 的 dll 只有 ~11 MB，
//! 模型 det 4.5 MB + rec 10.4 MB，整条 OCR 链路 26 MB 就能跑起来。
//!
//! 线程策略：intra_threads = min(核数, 4)。ONNX Runtime 默认会把所有物理核
//! 都开满，空闲时线程池本身就要几十 MB 栈空间，与"空闲 < 80 MB"冲突；
//! 而 OCR 是短任务，4 线程已经能把 2560×1440 全屏压到 300 ms 以内。

pub mod det;
pub mod rec;

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use image::RgbaImage;
use ort::environment::Environment;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use serde::Serialize;

use det::TextBox;

/// ONNX Runtime 的全局环境，必须比所有 Session 活得久。
///
/// 用 `Result` 包一层：dll 找不到时把错误缓存下来，界面能连续拿到同一句
/// 提示，而不是每次点识别都重新扫一遍磁盘。
static ORT_ENV: OnceLock<Result<Environment, String>> = OnceLock::new();

/// 引擎单例。
///
/// 用 `Option` 而不是直接放引擎，是为了能主动卸载：OCR 一旦加载，
/// ONNX Runtime 的会话 arena 与权重会常驻几十 MB，直接顶穿"空闲 < 80 MB"。
/// 面板销毁时调用 `unload()` 把它还回去，下次用到再加载（约 250 ms，
/// 但这个成本只在用户真的截图识别时付出）。
static ENGINE: OnceLock<Mutex<Option<OcrEngine>>> = OnceLock::new();

/// 一行识别结果，坐标是原图像素。
#[derive(Serialize, Clone, Debug)]
pub struct OcrLine {
    pub text: String,
    pub conf: f32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Serialize)]
pub struct OcrResult {
    /// 整页文本，按阅读顺序用换行拼接。
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub det_ms: u64,
    pub rec_ms: u64,
    pub total_ms: u64,
    /// 检测网络实际吃进去的分辨率，用来解释"为什么小字没识别出来"。
    pub det_size: [u32; 2],
}

pub struct OcrEngine {
    det: Session,
    rec: Session,
    keys: Vec<String>,
    /// rec 输入宽固定值（静态 shape 模型）。None 表示动态宽度。
    rec_fixed_w: Option<u32>,
    pub load_ms: u64,
}

/// 模型目录解析顺序：
/// 1. `SHOTGLANCE_MODELS`（调试/换模型用）
/// 2. exe 同级 `models/`（安装包 & 便携版）
/// 3. 编译期 manifest 下的 `models/`（`cargo run` / 自动化测试）
fn models_dir() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("SHOTGLANCE_MODELS") {
        let p = PathBuf::from(p);
        if p.join("det.onnx").exists() {
            return Ok(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Tauri 在 Windows 上把 resources 放在 exe 同级；macOS 在
            // Contents/Resources。两个位置都试一遍，省一套 cfg。
            for cand in [dir.join("models"), dir.join("../Resources/models")] {
                if cand.join("det.onnx").exists() {
                    return Ok(cand);
                }
            }
        }
    }

    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
    if dev.join("det.onnx").exists() {
        return Ok(dev);
    }

    Err("找不到模型目录（det.onnx / rec.onnx / dict.txt）".into())
}

/// 定位 onnxruntime.dll。
///
/// load-dynamic 模式下 dll 是我们自己带的分发文件，查找顺序要和打包布局对齐：
/// 1. `SHOTGLANCE_ORT_DLL`：显式指定（换版本/排障）
/// 2. exe 同级：安装包（Tauri 把 resources 放这里）
/// 3. exe 同级 models/：便携版把 dll 和模型放一起
/// 4. 仓库 `vendor/`：`cargo run` 与自动化测试
fn find_dylib() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };

    if let Some(p) = std::env::var_os("SHOTGLANCE_ORT_DLL") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
            candidates.push(dir.join("models").join(name));
            candidates.push(dir.join("../Resources").join(name));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor").join(name));

    candidates.into_iter().find(|p| p.is_file())
}

/// 进程级 ONNX Runtime 环境。翻译层也复用这一个实例：
/// Environment 是全局单例，加载两次 dll 会让两个 Session 分属不同环境，
/// 内存直接翻倍。
pub fn ort_env() -> Result<&'static Environment, String> {
    match ORT_ENV.get_or_init(|| {
        let dll = find_dylib().ok_or_else(|| "找不到 onnxruntime.dll".to_string())?;
        let env = ort::init_from(&dll)
            .map_err(|e| format!("加载 {} 失败: {e}", dll.display()))?
            .with_name("shotglance")
            .with_telemetry(false)
            .build()
            .map_err(|e| format!("初始化 ONNX Runtime 环境失败: {e}"))?;
        Ok(env)
    }) {
        Ok(e) => Ok(e),
        Err(e) => Err(e.clone()),
    }
}

fn load() -> Result<OcrEngine, String> {
    let started = Instant::now();
    let dir = models_dir()?;

    let dict = std::fs::read_to_string(dir.join("dict.txt"))
        .map_err(|e| format!("读取字典失败: {e}"))?;
    // 千万不能 trim 每一行：字典里存在以空格等不可见字符开头的条目，
    // trim 会让索引整体偏移一个字，导致整页文本系统性错位。
    let keys: Vec<String> = dict
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    if keys.len() < 3000 {
        return Err(format!("字典异常：只有 {} 条", keys.len()));
    }

    let env = ort_env()?;

    // 线程数可覆盖，方便在"时延 vs 内存"之间做实测取舍。
    // ONNX Runtime 每个 intra 线程都会带自己的 arena，线程数直接算进常驻内存。
    let threads = std::env::var("SHOTGLANCE_ORT_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .min(4)
        });

    let mut det = Session::builder(env)
        .map_err(|e| format!("创建 det builder 失败: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| format!("设置线程数失败: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("设置优化级别失败: {e}"))?
        .commit_from_file(dir.join("det.onnx"))
        .map_err(|e| format!("加载 det.onnx 失败: {e}"))?;

    // 检测模型只用一次前向，跑一遍热身可把"首次调用的内存分配 + 算子选择"
    // 从用户路径挪到加载路径（实测差 60–90 ms）。失败不影响后续真实推理。
    if let Ok(t) = Tensor::from_array(([1i64, 3, 32, 32], vec![0f32; 3 * 32 * 32])) {
        let _ = det.run(ort::inputs![t]);
    }

    let rec = Session::builder(env)
        .map_err(|e| format!("创建 rec builder 失败: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| format!("设置线程数失败: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("设置优化级别失败: {e}"))?
        .commit_from_file(dir.join("rec.onnx"))
        .map_err(|e| format!("加载 rec.onnx 失败: {e}"))?;

    Ok(OcrEngine {
        det,
        rec,
        keys,
        rec_fixed_w: None,
        load_ms: started.elapsed().as_millis() as u64,
    })
}

/// 取引擎，必要时加载。返回锁的守卫：`Session::run` 需要 `&mut`。
pub fn engine() -> Result<MutexGuard<'static, Option<OcrEngine>>, String> {
    let slot = ENGINE.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(load()?);
    }
    Ok(guard)
}

/// 卸载引擎，把 ONNX Runtime 的会话内存还给系统。
///
/// 只在面板被销毁时调用。加载成本 250 ms 左右，而"用户合上面板"通常意味着
/// 一段时间不会再用；用一次加载换几十 MB 常驻是划算的。
pub fn unload() {
    if let Some(slot) = ENGINE.get() {
        let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_some() {
            *guard = None;
        }
    }
}

/// 预检：模型与 dll 是否都在。前端用它在启动时提示具体缺什么，
/// 而不是等用户点完识别才看到报错。
pub fn preflight() -> Result<(), String> {
    models_dir()?;
    if find_dylib().is_none() {
        return Err("找不到 onnxruntime.dll（应与 exe 同级）".into());
    }
    Ok(())
}

pub fn recognize(img: &RgbaImage) -> Result<OcrResult, String> {
    let mut guard = engine()?;
    // engine() 保证了槽位非空，这里只是把 &mut 取出来。
    let engine = guard.as_mut().ok_or("OCR 引擎未加载")?;
    engine.run(img)
}

/// 单行识别置信度下限。
///
/// 0.5 是 PaddleOCR 的 `drop_score` 默认值。低于这个值的结果基本是
/// 排版噪声：边框、图标、半透明水印——它们会被检测网络当成"文本"，
/// 但识别网络只能吐出几个不相干的字。留着会污染整页文本。
const MIN_LINE_CONF: f32 = 0.5;

impl OcrEngine {
    fn run(&mut self, img: &RgbaImage) -> Result<OcrResult, String> {
        let total = Instant::now();

        // ---- 检测 ----
        let t = Instant::now();
        let det::DetInput {
            data,
            w: dw,
            h: dh,
            scale_x,
            scale_y,
        } = det::preprocess(img);
        let tensor = Tensor::from_array(([1i64, 3, dh as i64, dw as i64], data))
            .map_err(|e| format!("构造检测输入失败: {e}"))?;

        // 用块把 outputs 的作用域收紧：它借着 &mut self.det，
        // 只有等它析构后才能再借 self 去跑识别。
        let boxes = {
            let outputs = self
                .det
                .run(ort::inputs![tensor])
                .map_err(|e| format!("检测推理失败: {e}"))?;
            let (_shape, prob) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("读取检测输出失败: {e}"))?;
            det::postprocess(prob, dw, dh, scale_x, scale_y)
        };
        let det_ms = t.elapsed().as_millis() as u64;

        if boxes.is_empty() {
            return Ok(OcrResult {
                text: String::new(),
                lines: Vec::new(),
                det_ms,
                rec_ms: 0,
                total_ms: total.elapsed().as_millis() as u64,
                det_size: [dw, dh],
            });
        }

        // ---- 识别 ----
        // 逐行推理而不做批处理：批处理要把不同长度的行 padding 到同一宽度，
        // 短行会白白多算几倍面积；而行数在截图场景通常只有几十行，
        // 逐行的调度开销（约 0.1 ms/行）远小于 padding 的浪费。
        let t = Instant::now();
        let mut lines = Vec::with_capacity(boxes.len());
        for b in &boxes {
            match self.recognize_box(img, b) {
                Ok((text, conf)) => {
                    if text.trim().is_empty() || conf < MIN_LINE_CONF {
                        continue;
                    }
                    lines.push(OcrLine {
                        text,
                        conf,
                        x: b.x,
                        y: b.y,
                        w: b.w,
                        h: b.h,
                    });
                }
                Err(_) => continue, // 单行失败不拖垮整页
            }
        }
        let rec_ms = t.elapsed().as_millis() as u64;

        let text = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(OcrResult {
            text,
            lines,
            det_ms,
            rec_ms,
            total_ms: total.elapsed().as_millis() as u64,
            det_size: [dw, dh],
        })
    }

    fn recognize_box(&mut self, img: &RgbaImage, b: &TextBox) -> Result<(String, f32), String> {
        let crop = det::crop(img, b);

        // 宽度策略：优先用动态宽度（按纵横比算），模型若是静态 shape
        // 则第一次会失败，之后固定成该宽度重试——避免为了拿 shape
        // 元信息去折腾 ORT 的 ValueType 反射。
        let width = self.rec_fixed_w.unwrap_or_else(|| rec::width_for(&crop));
        let (text, conf) = match self.run_rec(&crop, width) {
            Ok(r) => r,
            Err(e) if self.rec_fixed_w.is_none() => {
                let fixed = 320;
                let r = self
                    .run_rec(&crop, fixed)
                    .map_err(|e2| format!("{e} / 回退 320 也失败: {e2}"))?;
                self.rec_fixed_w = Some(fixed);
                r
            }
            Err(e) => return Err(e),
        };
        Ok((text, conf))
    }

    fn run_rec(&mut self, crop: &RgbaImage, width: u32) -> Result<(String, f32), String> {
        let data = rec::preprocess(crop, width);
        let tensor = Tensor::from_array(([1i64, 3, rec::INPUT_H as i64, width as i64], data))
            .map_err(|e| format!("构造识别输入失败: {e}"))?;

        let outputs = self
            .rec
            .run(ort::inputs![tensor])
            .map_err(|e| format!("识别推理失败: {e}"))?;
        let (_shape, logits) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("读取识别输出失败: {e}"))?;

        // 输出形状 [1, T, C]，C = blank + 字典 + 空格。
        // 不用 Shape 反查，直接用字典长度推 C，再反算 T，少一层 API 耦合。
        let classes = self.keys.len() + 2;
        if logits.len() % classes != 0 {
            return Err(format!(
                "识别输出维度不符：len={} 不能被 classes={} 整除",
                logits.len(),
                classes
            ));
        }
        let steps = logits.len() / classes;
        let out = rec::decode(logits, steps, classes, &self.keys);
        Ok((out.text, out.conf))
    }
}
