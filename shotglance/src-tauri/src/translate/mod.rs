//! 翻译层：多引擎并存，用户在设置里切换。
//!
//! 设计约束（来自项目硬指标）：
//! 1. **离线优先**。在线引擎再好，没网就是零。所以 `auto` 的降级链是
//!    "先试在线、失败转离线"，而不是反过来。
//! 2. **引擎可插拔**。全部收敛到 `Engine` trait，新增一家 = 加一个文件 +
//!    在 `REGISTRY` 里加一行，不动调度逻辑。
//! 3. **密钥只落本地**。配置写在 `%APPDATA%/<app>/translate.json`，
//!    任何错误信息里都不回显密钥原文。
//!
//! `auto` 不是一个引擎，是降级链。界面上会显示"实际用了谁"和
//! "谁失败了、为什么"——不然用户没法判断该去改哪个开关。

pub mod offline;
pub mod online;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// 一次翻译请求。`source` 为 `auto` 时由各引擎自行判断（OCR 场景下
/// 源语言未知是常态）。
#[derive(Debug, Clone)]
pub struct Request {
    pub text: String,
    pub source: String,
    pub target: String,
}

/// 翻译结果。带引擎标识和耗时，方便界面上直接展示"谁翻的、花了多久"。
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub text: String,
    /// 实际生效的引擎 id
    pub engine: String,
    pub engine_label: String,
    /// 毫秒
    pub ms: u64,
    /// 从哪个引擎降级过来的（None 表示首选就成功）
    pub fell_back_from: Option<String>,
    /// 降级原因，界面上提示用
    pub note: Option<String>,
    /// 是否命中本地缓存（命中的话 ms 没有参考价值）
    pub cached: bool,
}

/// 引擎元信息，给前端渲染下拉框。
#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub id: String,
    pub label: String,
    /// online | offline
    pub kind: String,
    /// 当前配置下是否可用
    pub ready: bool,
    /// 不可用的原因，界面上灰掉并给 tooltip
    pub reason: Option<String>,
    /// 是否需要用户填密钥/端点
    pub needs_config: bool,
}

/// 用户配置。字段全部是字符串/布尔，序列化到 JSON 无坑。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Cfg {
    /// "auto" | "mymemory" | "google" | "deepl" | "openai" | "offline"
    pub engine: String,
    /// "auto" | "en" | "zh" | ...
    pub source: String,
    /// "zh-CN" | "en" | ...
    pub target: String,

    /// MyMemory 支持可选 email，带上配额更高（匿名每天 5000 字符）
    pub mymemory_email: String,

    /// DeepL 密钥。free 档端点以 :fx 结尾，pro 档是 api.deepl.com
    pub deepl_key: String,
    pub deepl_pro: bool,

    /// OpenAI 兼容端点。默认指向 OpenAI 官方，用户可换成任何兼容服务
    /// （DeepSeek / 通义 / 本地 Ollama / one-api 中转）
    pub openai_base: String,
    pub openai_key: String,
    pub openai_model: String,

    /// 离线引擎单句最大 token 数。太长会拖慢自回归解码
    pub offline_max_tokens: usize,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            engine: "auto".into(),
            source: "auto".into(),
            target: "zh-CN".into(),
            mymemory_email: String::new(),
            deepl_key: String::new(),
            deepl_pro: false,
            openai_base: "https://api.openai.com/v1".into(),
            openai_key: String::new(),
            openai_model: "gpt-4o-mini".into(),
            offline_max_tokens: 96,
        }
    }
}

/// 引擎契约。
///
/// 同步接口：截图 OCR 本身是阻塞流水线，命令层统一用 `spawn_blocking`
/// 包一层就够了，没必要为了"看起来异步"在每个引擎里引入异步运行时。
pub trait Engine: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// online | offline
    fn kind(&self) -> &'static str;
    /// 需要用户配置才可用？
    fn needs_config(&self) -> bool {
        false
    }
    /// 当前配置下能不能用。Err 里是给人看的原因。
    fn ready(&self, cfg: &Cfg) -> Result<(), String>;
    /// 真正干活。`text` 已经是分好段的一片。
    fn translate(&self, cfg: &Cfg, req: &Request) -> Result<String, String>;
}

/// 引擎表。顺序即 `auto` 的优先级：
/// 先看用户配过的付费引擎（配了 key 才有意义），再是免密钥的质量档，
/// 最后兜底离线。没配 key 的引擎 `ready()` 会直接返回 Err，跳过不花钱。
fn registry() -> &'static [&'static dyn Engine] {
    static REG: OnceLock<Vec<&'static dyn Engine>> = OnceLock::new();
    REG.get_or_init(|| {
        vec![
            &online::DeepL,
            &online::OpenAi,
            &online::Google,
            &online::MyMemory,
            &offline::Offline,
        ]
    })
}

pub fn find(id: &str) -> Option<&'static dyn Engine> {
    registry().iter().copied().find(|e| e.id() == id)
}

pub fn engines(cfg: &Cfg) -> Vec<EngineInfo> {
    registry()
        .iter()
        .map(|e| {
            let r = e.ready(cfg);
            EngineInfo {
                id: e.id().to_string(),
                label: e.label().to_string(),
                kind: e.kind().to_string(),
                ready: r.is_ok(),
                reason: r.err(),
                needs_config: e.needs_config(),
            }
        })
        .collect()
}

/* ---------------- 配置持久化 ---------------- */

fn cfg_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("拿不到配置目录：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("建配置目录失败：{e}"))?;
    Ok(dir.join("translate.json"))
}

pub fn load_cfg(app: &AppHandle) -> Cfg {
    // 读失败就回默认值：翻译配置坏了不该让整个应用起不来。
    cfg_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_cfg(app: &AppHandle, cfg: &Cfg) -> Result<(), String> {
    let p = cfg_path(app)?;
    let s = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写配置失败：{e}"))
}

/* ---------------- 缓存 ---------------- */

/// 结果缓存。截图翻译有很强的重复性（同一段文字反复框），
/// 命中一次就省掉一次往返，离线引擎更是省掉几百毫秒解码。
fn cache() -> &'static Mutex<HashMap<String, String>> {
    static C: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const CACHE_MAX: usize = 256;

fn cache_key(engine: &str, req: &Request) -> String {
    format!("{}\u{1}{}\u{1}{}\u{1}{}", engine, req.source, req.target, req.text)
}

fn cache_get(k: &str) -> Option<String> {
    cache().lock().ok()?.get(k).cloned()
}

fn cache_put(k: String, v: String) {
    if let Ok(mut m) = cache().lock() {
        // 简单淘汰：满了就清空。翻译缓存不是热点数据，不值得为它引入 LRU。
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        m.insert(k, v);
    }
}

/* ---------------- 失败冷却 ---------------- */

/// 某个在线引擎刚失败过，短时间内在 auto 链里跳过它。
///
/// 没有这个机制，网络不通时（本机 googleapis 就是直接连不上）每次翻译
/// 都要先卡满一个连接超时，用户会以为整个功能坏了。90 秒足够让一次
/// 手滑的断网恢复过来，又不至于让用户换好网络后还等太久。
const COOLDOWN: Duration = Duration::from_secs(90);

fn cooldown() -> &'static Mutex<HashMap<&'static str, Instant>> {
    static C: OnceLock<Mutex<HashMap<&'static str, Instant>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn in_cooldown(id: &str) -> bool {
    cooldown()
        .lock()
        .ok()
        .and_then(|m| m.get(id).map(|t| t.elapsed() < COOLDOWN))
        .unwrap_or(false)
}

fn note_fail(id: &'static str) {
    if let Ok(mut m) = cooldown().lock() {
        m.insert(id, Instant::now());
    }
}

fn note_ok(id: &'static str) {
    if let Ok(mut m) = cooldown().lock() {
        m.remove(id);
    }
}

/* ---------------- 分段 ---------------- */

/// 把长文本切成若干片，每片不超过 `max_chars` 个字符。
///
/// 先按行切，单行超长再按句末标点切，最后兜底硬切。
/// 保留原有的 `\n`：译文按行对齐时，换行结构是唯一能用的线索。
pub fn chunk(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let mut out = Vec::new();
    let mut cur = String::new();

    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            out.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };

    for line in text.split_inclusive('\n') {
        if line.chars().count() > max_chars {
            // 超长单行：先冲掉手上的，再把这一行按标点拆
            flush(&mut cur, &mut out);
            for piece in split_long_line(line, max_chars) {
                out.push(piece);
            }
            continue;
        }
        if cur.chars().count() + line.chars().count() > max_chars {
            flush(&mut cur, &mut out);
        }
        cur.push_str(line);
    }
    flush(&mut cur, &mut out);

    if out.is_empty() {
        out.push(text.to_string());
    }
    out
}

fn split_long_line(line: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    // 句末标点是天然的安全切点：在句中断开会让机器翻译丢上下文
    for ch in line.chars() {
        cur.push(ch);
        let boundary = matches!(ch, '.' | '!' | '?' | ';' | '。' | '！' | '？' | '；' | '\n');
        if boundary && cur.chars().count() >= max_chars / 2 {
            out.push(std::mem::take(&mut cur));
        } else if cur.chars().count() >= max_chars {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/* ---------------- HTTP ---------------- */

/// native-tls 必须显式注入。
///
/// ureq 只把开启了 `tls`(rustls) feature 的后端当默认；`native-tls` feature
/// 仅仅让它"可用"，永远不会被自动采用。漏掉这一步，所有 https 请求都会
/// 以 `no TLS backend is configured` 失败——而这个报错看起来像网络问题，
/// 极容易查错方向。
fn tls_connector() -> Option<std::sync::Arc<ureq::native_tls::TlsConnector>> {
    ureq::native_tls::TlsConnector::builder()
        .build()
        .ok()
        .map(std::sync::Arc::new)
}

/// 复用一个 Agent：TLS 握手是百毫秒级的开销，每次新建连接会把它付在
/// 每一次翻译上。`try_proxy_from_env` 让 HTTP(S)_PROXY 生效——
/// 本机实测 Google 直连不通，走代理才通。
pub fn agent() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| {
        let mut b = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .try_proxy_from_env(true);
        if let Some(t) = tls_connector() {
            b = b.tls_connector(t);
        }
        b.build()
    })
}

/// 下载模型专用的 Agent。
///
/// 全局那个 20 秒总超时对 60 MB 的下载是致命的，这里只限制
/// "多久没收到数据"，不限总时长。
pub fn download_agent() -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(120))
        .try_proxy_from_env(true);
    if let Some(t) = tls_connector() {
        b = b.tls_connector(t);
    }
    b.build()
}

/// 把 ureq 的错误压成一句人话。界面上直接显示，所以要具体：
/// 超时、DNS、证书、HTTP 状态码分别对应完全不同的处理动作。
pub fn describe_http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let text = resp.into_string().unwrap_or_default();
            let body: String = text.trim().chars().take(160).collect();
            if body.is_empty() {
                format!("HTTP {code}")
            } else {
                format!("HTTP {code}：{body}")
            }
        }
        ureq::Error::Transport(t) => {
            let msg = t.to_string();
            if msg.contains("timed out") || msg.contains("timeout") {
                "连接超时（可能需要代理）".to_string()
            } else if msg.contains("dns") || msg.contains("resolve") {
                "域名解析失败（检查网络或代理）".to_string()
            } else if msg.contains("certificate") || msg.contains("tls") {
                format!("TLS 握手失败：{msg}")
            } else {
                format!("网络错误：{msg}")
            }
        }
    }
}

/* ---------------- 调度 ---------------- */

/// 翻译入口。用当前保存的配置。
pub fn run(app: &AppHandle, text: &str) -> Result<Outcome, String> {
    run_with(&load_cfg(app), text)
}

/// 用指定配置翻译。`auto` 走降级链，指定引擎则只试那一个。
pub fn run_with(cfg: &Cfg, text: &str) -> Result<Outcome, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Outcome {
            text: String::new(),
            engine: "none".into(),
            engine_label: "—".into(),
            ms: 0,
            fell_back_from: None,
            note: None,
            cached: true,
        });
    }

    let req = Request {
        text: text.to_string(),
        source: cfg.source.clone(),
        target: cfg.target.clone(),
    };

    let key = cache_key(&cfg.engine, &req);
    if let Some(hit) = cache_get(&key) {
        let label = find(&cfg.engine).map(|e| e.label()).unwrap_or("缓存");
        return Ok(Outcome {
            text: hit,
            engine: cfg.engine.clone(),
            engine_label: label.to_string(),
            ms: 0,
            fell_back_from: None,
            note: None,
            cached: true,
        });
    }

    // auto：在线引擎对冲竞速，全挂了再串行落到离线模型。
    // 指定引擎：只跑那一个，失败就是失败——用户在这里做的是明确选择，
    // 悄悄换成别的引擎反而会让人怀疑"为什么译文风格变了"。
    if cfg.engine == "auto" {
        let online: Vec<&'static dyn Engine> = registry()
            .iter()
            .copied()
            .filter(|e| e.kind() == "online")
            .collect();

        let (hit, mut fails) = race(cfg, &req, online);
        if let Some(o) = hit {
            cache_put(key, o.text.clone());
            return Ok(o);
        }

        if let Some(off) = find("offline") {
            match off.ready(cfg) {
                Err(r) => fails.push(format!("{}：{}", off.label(), r)),
                Ok(()) => {
                    let started = Instant::now();
                    match translate_chunked(off, cfg, &req) {
                        Ok(t) if !t.trim().is_empty() => {
                            let ms = started.elapsed().as_millis() as u64;
                            cache_put(key, t.clone());
                            return Ok(Outcome {
                                text: t,
                                engine: off.id().to_string(),
                                engine_label: off.label().to_string(),
                                ms,
                                fell_back_from: Some("online".into()),
                                note: Some(fails.join("；")).filter(|s| !s.is_empty()),
                                cached: false,
                            });
                        }
                        Ok(_) => fails.push(format!("{}：返回空译文", off.label())),
                        Err(e) => fails.push(format!("{}：{}", off.label(), e)),
                    }
                }
            }
        }

        return Err(if fails.is_empty() {
            "没有可用的翻译引擎".to_string()
        } else {
            fails.join("
")
        });
    }

    let engine = find(&cfg.engine).ok_or_else(|| format!("未知引擎：{}", cfg.engine))?;
    engine
        .ready(cfg)
        .map_err(|e| format!("{}：{}", engine.label(), e))?;

    let started = Instant::now();
    let out = translate_chunked(engine, cfg, &req)?;
    let ms = started.elapsed().as_millis() as u64;
    note_ok(engine.id());
    cache_put(key, out.clone());
    Ok(Outcome {
        text: out,
        engine: engine.id().to_string(),
        engine_label: engine.label().to_string(),
        ms,
        fell_back_from: None,
        note: None,
        cached: false,
    })
}

/// 对冲竞速（hedged request）。
///
/// 第一个可用引擎立刻发出；之后每 250 ms 追加一个，谁先给出有效译文就用谁，
/// 其余线程的结果直接丢弃。
///
/// 为什么不是"全部同时发"：MyMemory 的免费额度按字符算（匿名 5000 字符/天），
/// 无脑并发会把它白白烧光。对冲在正常网络下只有第一个引擎真正发出请求，
/// 只有在它慢到超过 250 ms 时才补发——恰好覆盖"某些端点在这台机器上
/// 根本不连通"的情况。
fn race(
    cfg: &Cfg,
    req: &Request,
    engines: Vec<&'static dyn Engine>,
) -> (Option<Outcome>, Vec<String>) {
    use std::sync::mpsc;

    let mut fails: Vec<String> = Vec::new();
    let mut usable: Vec<&'static dyn Engine> = Vec::new();
    for e in engines {
        match e.ready(cfg) {
            Err(r) => fails.push(format!("{}：{}", e.label(), r)),
            Ok(()) if in_cooldown(e.id()) => fails.push(format!(
                "{}：刚刚失败过，{} 秒内先跳过",
                e.label(),
                COOLDOWN.as_secs()
            )),
            Ok(()) => usable.push(e),
        }
    }
    if usable.is_empty() {
        return (None, fails);
    }

    let (tx, rx) = mpsc::channel();
    for (i, e) in usable.iter().enumerate() {
        let tx = tx.clone();
        let cfg = cfg.clone();
        let req = req.clone();
        let engine: &'static dyn Engine = *e;
        std::thread::spawn(move || {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(250 * i as u64));
            }
            let started = Instant::now();
            let r = translate_chunked(engine, &cfg, &req);
            let _ = tx.send((
                engine.id(),
                engine.label(),
                r,
                started.elapsed().as_millis() as u64,
            ));
        });
    }
    drop(tx);

    let n = usable.len();
    let mut first_fail: Option<&'static str> = None;
    for _ in 0..n {
        // 单个引擎的 HTTP 超时是 20 秒，这里放宽到 30 秒是为了让
        // "最慢的那个"有机会报出自己的真实错误，而不是被我们提前掐断。
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok((id, label, res, ms)) => match res {
                Ok(t) if !t.trim().is_empty() => {
                    note_ok(id);
                    return (
                        Some(Outcome {
                            text: t,
                            engine: id.to_string(),
                            engine_label: label.to_string(),
                            ms,
                            fell_back_from: first_fail.map(|s| s.to_string()),
                            note: Some(fails.join("；")).filter(|s| !s.is_empty()),
                            cached: false,
                        }),
                        fails,
                    );
                }
                Ok(_) => {
                    note_fail(id);
                    first_fail.get_or_insert(id);
                    fails.push(format!("{label}：返回空译文"));
                }
                Err(e) => {
                    note_fail(id);
                    first_fail.get_or_insert(id);
                    fails.push(format!("{label}：{e}"));
                }
            },
            Err(_) => {
                fails.push("在线引擎全部超时".to_string());
                break;
            }
        }
    }
    (None, fails)
}

/// 长文本按引擎能力分段，逐段翻译后拼回。
fn translate_chunked(
    engine: &'static dyn Engine,
    cfg: &Cfg,
    req: &Request,
) -> Result<String, String> {
    // 各引擎的单次上限不同，取保守值。OpenAI 一次能吃很多，
    // 但拆段对成本更低、失败重试也更便宜，所以统一按 1800 字符切。
    let limit = if engine.id() == "mymemory" {
        // MyMemory 匿名配额按字符算，单次超过 500 字节会直接拒绝
        400
    } else {
        1800
    };

    let pieces = chunk(&req.text, limit);
    if pieces.len() == 1 {
        return engine.translate(cfg, req);
    }

    let mut out = String::new();
    for p in &pieces {
        let sub = Request {
            text: p.clone(),
            source: req.source.clone(),
            target: req.target.clone(),
        };
        let t = engine.translate(cfg, &sub)?;
        out.push_str(&t);
        // 分段时保留了换行，这里只在原片段不以换行结尾时补一个，
        // 避免把原本连续的句子粘成一行。
        if !p.ends_with('\n') && !t.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out.trim_end().to_string())
}
