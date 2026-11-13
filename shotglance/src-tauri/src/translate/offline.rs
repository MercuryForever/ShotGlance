//! 离线翻译引擎：Opus-MT (Marian) encoder-decoder，ONNX Runtime 推理。
//!
//! 为什么自己写分词而不是引 tokenizers / sentencepiece：
//! - sentencepiece crate 要 cmake + protobuf 编译 C++，给 20 MB 的安装包
//!   加这种构建依赖不划算，而且这台机器上没有 cmake。
//! - tokenizers crate 需要先有 tokenizer.json，而 Xenova 的仓库只有 .spm。
//! 所以改成构建期把 spm 的分数和 vocab.json 的 id 合成 tokens.tsv，
//! 运行时只做 Viterbi —— 逻辑不到 100 行，且完全可测。
//!
//! 解码用贪婪而不是官方默认的 beam=4：实测 beam 慢 4 倍，而长文本
//! （真实使用场景）质量差别很小，短句的差异主要来自模型本身而非搜索策略。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;

use super::{Cfg, Engine, Request};

/// Marian 的特殊 token 约定（来自 config.json，与 spm 的 id 不是一回事）
const PAD: u32 = 65000;
const EOS: u32 = 0;
const UNK: u32 = 1;
/// 模型 vocab 大小，用于从展平的 logits 里定位最后一帧
const VOCAB: usize = 65001;
/// 位置编码上限
const MAX_POS: usize = 512;

pub struct Offline;

/* ---------------- 模型目录 ---------------- */

/// 模型存放位置。
///
/// 放 `%APPDATA%` 而不是安装目录：113 MB 的模型如果打进安装包，
/// 装包会从 21 MB 涨到 135 MB，直接顶到 150 MB 的红线；而且它只对
/// 选了离线引擎的用户有价值。放用户目录还顺带解决了免管理员权限的问题。
pub fn models_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SHOTGLANCE_MT_DIR") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("ShotGlance").join("models").join("opus-mt-en-zh")
}

fn file_ok(p: &Path, min_bytes: u64) -> bool {
    p.metadata().map(|m| m.len() >= min_bytes).unwrap_or(false)
}

/// 三个文件齐了才算可用。阈值给得比实际大小低一截，只防"下了一半"。
pub fn installed() -> bool {
    let d = models_dir();
    file_ok(&d.join("encoder.onnx"), 40 * 1024 * 1024)
        && file_ok(&d.join("decoder.onnx"), 45 * 1024 * 1024)
        && file_ok(&d.join("tokens.tsv"), 1024 * 1024)
}

/// 给界面用的模型状态。
#[derive(serde::Serialize)]
pub struct ModelStatus {
    pub installed: bool,
    pub dir: String,
    pub bytes: u64,
}

pub fn status() -> ModelStatus {
    let d = models_dir();
    let bytes = ["encoder.onnx", "decoder.onnx", "tokens.tsv"]
        .iter()
        .filter_map(|f| d.join(f).metadata().ok().map(|m| m.len()))
        .sum();
    ModelStatus {
        installed: installed(),
        dir: d.display().to_string(),
        bytes,
    }
}

/* ---------------- 词表 ---------------- */

struct Node {
    /// 字符 → 子节点下标
    next: HashMap<char, u32>,
    /// 该节点结尾的模型 id，无词为 -1
    id: i32,
    score: f32,
}

/// 词表 + 前缀树。65001 个 piece 全部进树，节点数约 25 万，
/// 构建耗时 100 ms 量级，只在模型加载时做一次。
struct Vocab {
    nodes: Vec<Node>,
    /// 解码用：模型 id → piece
    pieces: Vec<Option<String>>,
    /// 无法匹配任何 piece 时的兜底分数。
    /// spm 内部是 `min_score - 10`，这里照做：不压到最低的话，
    /// Viterbi 会用 unk 拼出更"高分"的路径，输出全是 unk。
    unk_score: f32,
}

impl Vocab {
    fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 tokens.tsv 失败：{e}"))?;

        let mut nodes = vec![Node {
            next: HashMap::new(),
            id: -1,
            score: 0.0,
        }];
        let mut pieces: Vec<Option<String>> = vec![None; VOCAB];
        let mut min_score = 0f32;

        for line in text.lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(id_s), Some(sc_s), Some(piece)) =
                (it.next(), it.next(), it.next())
            else {
                continue;
            };
            let (Ok(id), Ok(score)) = (id_s.parse::<usize>(), sc_s.parse::<f32>()) else {
                continue;
            };
            if id >= VOCAB {
                continue;
            }

            pieces[id] = Some(piece.to_string());
            if score < min_score {
                min_score = score;
            }

            // 只有带真实分数的 piece 才进树。vocab.json 里那 33000 个
            // spm 未保留的 piece 分数是 -100，让它们进树只会拖慢匹配。
            if score <= -99.0 {
                continue;
            }

            let mut cur = 0usize;
            for ch in piece.chars() {
                let nxt = match nodes[cur].next.get(&ch) {
                    Some(&x) => x as usize,
                    None => {
                        nodes.push(Node {
                            next: HashMap::new(),
                            id: -1,
                            score: 0.0,
                        });
                        let idx = nodes.len() - 1;
                        nodes[cur].next.insert(ch, idx as u32);
                        idx
                    }
                };
                cur = nxt;
            }
            nodes[cur].id = id as i32;
            nodes[cur].score = score;
        }

        if pieces.iter().filter(|p| p.is_some()).count() < 30000 {
            return Err("词表异常：有效 piece 太少".into());
        }

        Ok(Self {
            nodes,
            pieces,
            unk_score: min_score - 10.0,
        })
    }

    /// SentencePiece 的预处理：空白折叠成 ▁，并在开头补一个 ▁
    /// （等价于 add_dummy_prefix，让句首词和句中词用同一个 piece）。
    fn prepare(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 1);
        out.push('\u{2581}');
        let mut prev_space = true;
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !prev_space {
                    out.push('\u{2581}');
                    prev_space = true;
                }
            } else {
                out.push(ch);
                prev_space = false;
            }
        }
        out
    }

    /// Unigram Viterbi：在词图上找总对数概率最高的切分。
    fn encode(&self, text: &str) -> Vec<u32> {
        let prepared = Self::prepare(text);
        let chars: Vec<char> = prepared.chars().collect();
        let n = chars.len();
        if n == 0 {
            return Vec::new();
        }

        let mut best = vec![f32::NEG_INFINITY; n + 1];
        let mut back = vec![(0usize, UNK); n + 1];
        let mut reach = vec![false; n + 1];
        best[0] = 0.0;
        reach[0] = true;

        for i in 0..n {
            if !reach[i] {
                continue;
            }
            let base = best[i];
            let mut cur = 0usize;
            for j in i..n {
                let Some(&nx) = self.nodes[cur].next.get(&chars[j]) else {
                    break;
                };
                cur = nx as usize;
                let node = &self.nodes[cur];
                if node.id >= 0 {
                    let sc = base + node.score;
                    if sc > best[j + 1] {
                        best[j + 1] = sc;
                        back[j + 1] = (i, node.id as u32);
                        reach[j + 1] = true;
                    }
                }
            }
        }

        // 有字符一个 piece 都匹配不上（罕见词、OCR 噪声）时用 unk 单字兜底，
        // 保证整条路径连通——否则回溯会在断点处死循环。
        for i in 1..=n {
            if !reach[i] {
                best[i] = best[i - 1] + self.unk_score;
                back[i] = (i - 1, UNK);
                reach[i] = true;
            }
        }

        let mut ids = Vec::with_capacity(n / 2);
        let mut p = n;
        while p > 0 {
            let (prev, id) = back[p];
            ids.push(id);
            p = prev;
        }
        ids.reverse();
        ids
    }

    fn decode(&self, ids: &[u32]) -> String {
        let mut s = String::new();
        for &id in ids {
            if let Some(Some(p)) = self.pieces.get(id as usize) {
                s.push_str(p);
            }
        }
        polish(&s.replace('\u{2581}', " "))
    }
}

/* ---------------- 译文后处理 ---------------- */

fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

const ZH_PUNCT: &str = "，。！？；：、（）【】《》「」『』…—";

/// 收拾机器翻译的中文排版。
///
/// spm 解码后每个词之间是空格，中文里这些空格全都不该存在；
/// 模型还经常输出半角标点（`,` `?`），在中文语境下要转成全角。
/// 只对含中文的译文做，纯英文输出保持原样。
fn polish(s: &str) -> String {
    if !s.chars().any(is_cjk) {
        return s.trim().to_string();
    }

    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut prev: Option<char> = None;

    for i in 0..chars.len() {
        let c = chars[i];
        let nxt = chars.get(i + 1).copied();

        if c == ' ' {
            let drop = match nxt {
                // 标点前不留空格
                Some(n) if ZH_PUNCT.contains(n) => true,
                // 中文之间不留空格
                Some(n) if is_cjk(n) && prev.map(is_cjk).unwrap_or(false) => true,
                _ => false,
            };
            if drop {
                continue;
            }
        }

        // 半角标点紧跟在中文后面 → 转全角
        let c = match c {
            ',' if prev.map(is_cjk).unwrap_or(false) => '，',
            '?' if prev.map(is_cjk).unwrap_or(false) => '？',
            '!' if prev.map(is_cjk).unwrap_or(false) => '！',
            ';' if prev.map(is_cjk).unwrap_or(false) => '；',
            ':' if prev.map(is_cjk).unwrap_or(false) => '：',
            other => other,
        };

        out.push(c);
        if c != ' ' {
            prev = Some(c);
        }
    }

    // 全角标点后面若还有空格，一并去掉
    let mut cleaned = String::with_capacity(out.len());
    let mut it = out.chars().peekable();
    while let Some(c) = it.next() {
        cleaned.push(c);
        if ZH_PUNCT.contains(c) && it.peek() == Some(&' ') {
            it.next();
        }
    }
    cleaned.trim().to_string()
}

/* ---------------- 推理 ---------------- */

struct Model {
    enc: Session,
    dec: Session,
    vocab: Vocab,
}

fn slot() -> &'static Mutex<Option<Model>> {
    static S: OnceLock<Mutex<Option<Model>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn load() -> Result<Model, String> {
    let dir = models_dir();

    let vocab = Vocab::load(&dir.join("tokens.tsv"))?;

    // 和 OCR 共用同一个 ONNX Runtime 环境：Environment 是进程级单例，
    // 加载两次 dll 会让两个 Session 分属不同环境，内存直接翻倍。
    let env = crate::ocr::ort_env()?;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(4);

    let mk = |name: &str| -> Result<Session, String> {
        Session::builder(env)
            .map_err(|e| format!("创建 {name} builder 失败：{e}"))?
            .with_intra_threads(threads)
            .map_err(|e| format!("设置线程数失败：{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("设置优化级别失败：{e}"))?
            .commit_from_file(dir.join(name))
            .map_err(|e| format!("加载 {name} 失败：{e}"))
    };

    let enc = mk("encoder.onnx")?;
    let dec = mk("decoder.onnx")?;

    Ok(Model { enc, dec, vocab })
}

fn with_model<T>(f: impl FnOnce(&mut Model) -> Result<T, String>) -> Result<T, String> {
    let mut guard = slot().lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        *guard = Some(load()?);
    }
    f(guard.as_mut().expect("刚刚才装载"))
}

impl Engine for Offline {
    fn id(&self) -> &'static str {
        "offline"
    }
    fn label(&self) -> &'static str {
        "本地模型（离线）"
    }
    fn kind(&self) -> &'static str {
        "offline"
    }

    fn ready(&self, _cfg: &Cfg) -> Result<(), String> {
        if installed() {
            Ok(())
        } else {
            Err("模型未下载".into())
        }
    }

    fn translate(&self, cfg: &Cfg, req: &Request) -> Result<String, String> {
        if !installed() {
            return Err("模型未下载".into());
        }
        let max_new = cfg.offline_max_tokens.clamp(16, 256);

        with_model(|m| {
            let ids = m.vocab.encode(&req.text);
            if ids.is_empty() {
                return Ok(String::new());
            }
            // 位置编码上限 512，留一个位置给末尾的 </s>
            let len = ids.len().min(MAX_POS - 1);
            let mut src: Vec<i64> = ids[..len].iter().map(|&x| x as i64).collect();
            // Marian 要求源序列以 </s> 收尾，否则长句断句明显变差
            src.push(EOS as i64);
            let len = src.len();
            let mask: Vec<i64> = vec![1; len];

            let t_ids = Tensor::from_array(([1i64, len as i64], src))
                .map_err(|e| format!("构造 encoder 输入失败：{e}"))?;
            let t_mask = Tensor::from_array(([1i64, len as i64], mask.clone()))
                .map_err(|e| format!("构造 mask 失败：{e}"))?;

            // 输入顺序取自模型定义（encoder: input_ids, attention_mask），
            // 不能用名字传，rc.15 的 inputs! 只认位置。
            let enc_out = m
                .enc
                .run(ort::inputs![t_ids, t_mask])
                .map_err(|e| format!("encoder 推理失败：{e}"))?;
            let (h_shape, hidden) = enc_out[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("读取 encoder 输出失败：{e}"))?;
            let hidden: Vec<f32> = hidden.to_vec();
            // 用真实输出形状而不是硬编码 512，换模型时不会错位
            let h_dims: Vec<i64> = h_shape.iter().map(|&d| d as i64).collect();
            let h_shape_arr = [
                *h_dims.first().unwrap_or(&1),
                *h_dims.get(1).unwrap_or(&(len as i64)),
                *h_dims.get(2).unwrap_or(&512),
            ];

            let mut cur: Vec<i64> = vec![PAD as i64];
            let mut out: Vec<u32> = Vec::with_capacity(max_new);

            for _ in 0..max_new {
                let seq = cur.len() as i64;
                let t_in = Tensor::from_array(([1i64, seq], cur.clone()))
                    .map_err(|e| format!("构造 decoder 输入失败：{e}"))?;
                let t_m = Tensor::from_array(([1i64, len as i64], mask.clone()))
                    .map_err(|e| format!("构造 decoder mask 失败：{e}"))?;
                let t_h = Tensor::from_array((h_shape_arr, hidden.clone()))
                    .map_err(|e| format!("构造 hidden 失败：{e}"))?;

                // 顺序取自模型定义（decoder: encoder_attention_mask,
                // input_ids, encoder_hidden_states）
                let out_val = m
                    .dec
                    .run(ort::inputs![t_m, t_in, t_h])
                    .map_err(|e| format!("decoder 推理失败：{e}"))?;
                let (_, logits) = out_val[0]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("读取 decoder 输出失败：{e}"))?;

                // 只关心最后一帧。[1, seq, VOCAB] 展平后最后 VOCAB 个数就是它
                let start = (seq as usize - 1) * VOCAB;
                if logits.len() < start + VOCAB {
                    return Err(format!(
                        "decoder 输出维度不符：len={} 期望至少 {}",
                        logits.len(),
                        start + VOCAB
                    ));
                }
                let row = &logits[start..start + VOCAB];

                let mut best = f32::NEG_INFINITY;
                let mut best_id = EOS;
                for (i, &v) in row.iter().enumerate() {
                    // bad_words_ids = [[65000]]：pad 永远不该被生成
                    if i as u32 == PAD {
                        continue;
                    }
                    if v > best {
                        best = v;
                        best_id = i as u32;
                    }
                }

                if best_id == EOS {
                    break;
                }
                cur.push(best_id as i64);
                out.push(best_id);
            }

            Ok(m.vocab.decode(&out))
        })
    }
}

/// 面板销毁时释放模型，把一百多 MB 还给系统。
pub fn unload() {
    if let Ok(mut g) = slot().lock() {
        *g = None;
    }
}

/* ---------------- 下载 ---------------- */

/// 要从镜像拉的两个 ONNX。词表不打这里——它只有 1.8 MB 且和模型版本绑死，
/// 随安装包分发（见 `ensure_vocab`），用户点下载时只付模型的体积。
const FILES: &[(&str, &str, u64)] = &[
    (
        "encoder.onnx",
        "https://hf-mirror.com/Xenova/opus-mt-en-zh/resolve/main/onnx/encoder_model_quantized.onnx",
        52_899_742,
    ),
    (
        "decoder.onnx",
        "https://hf-mirror.com/Xenova/opus-mt-en-zh/resolve/main/onnx/decoder_model_quantized.onnx",
        60_212_804,
    ),
];

#[derive(Clone, serde::Serialize)]
pub struct Progress {
    pub file: String,
    pub done: u64,
    pub total: u64,
    pub overall: u64,
    pub overall_total: u64,
}

/// 把随包的词表铺到模型目录。下载和手动放置两条路都靠它保证词表就位。
pub fn ensure_vocab(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::path::BaseDirectory;
    use tauri::Manager;

    let dir = models_dir();
    let dst = dir.join("tokens.tsv");
    if dst.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("建模型目录失败：{e}"))?;
    let src = app
        .path()
        .resolve("mt/tokens.tsv", BaseDirectory::Resource)
        .map_err(|e| format!("定位内置词表失败：{e}"))?;
    std::fs::copy(&src, &dst).map_err(|e| format!("铺设词表失败：{e}"))?;
    Ok(())
}

/// 顺序下载两个模型文件，边下边发进度事件。
///
/// 阻塞调用，命令层用 `spawn_blocking` 包起来。写 `.part` 再 rename：
/// 中途断网留下的半截文件如果直接叫 `encoder.onnx`，下次启动会被
/// `installed()` 的尺寸检查挡掉；但如果尺寸刚好够（截断点靠后），
/// 就会加载出一个损坏的 Session。rename 是原子的，能彻底避免这种半成品。
pub fn download(app: &tauri::AppHandle) -> Result<(), String> {
    use std::io::{Read, Write};
    use tauri::Emitter;

    ensure_vocab(app)?;
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建模型目录失败：{e}"))?;

    // 独立的 agent：全局那个 20 秒总超时对 60 MB 的下载是致命的。
    let dl = super::download_agent();

    let overall_total: u64 = FILES.iter().map(|f| f.2).sum();
    let mut overall = 0u64;

    for (name, url, total) in FILES {
        let part = dir.join(format!("{name}.part"));
        let resp = dl.get(url).call().map_err(|e| super::describe_http_err(e))?;
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&part)
            .map_err(|e| format!("创建 {} 失败：{e}", part.display()))?;

        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        let mut last_emit = 0u64;
        let last_overall = overall;

        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("下载 {name} 中断：{e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("写入 {name} 失败：{e}"))?;
            done += n as u64;

            // 每 512 KB 发一次就够界面平滑了。每读一块就 emit 会把
            // 主线程的 IPC 队列淹掉，反而让进度条卡顿。
            if done - last_emit >= 512 * 1024 {
                last_emit = done;
                let ov = last_overall + done;
                let _ = app.emit(
                    "mt-progress",
                    Progress {
                        file: name.to_string(),
                        done,
                        total: *total,
                        overall: ov,
                        overall_total,
                    },
                );
            }
        }

        file.flush().ok();
        drop(file);
        overall = last_overall + done;

        if done < total * 95 / 100 {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "{name} 下载不完整：{done} / {total} 字节，请重试"
            ));
        }
        std::fs::rename(&part, dir.join(name))
            .map_err(|e| format!("落盘 {name} 失败：{e}"))?;
    }

    let _ = app.emit(
        "mt-progress",
        Progress {
            file: "done".into(),
            done: overall,
            total: overall_total,
            overall,
            overall_total,
        },
    );

    // 换模型后旧 Session 必须丢掉，否则还在用上一份文件
    unload();
    Ok(())
}
