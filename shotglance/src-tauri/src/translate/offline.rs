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

use ort::session::{builder::GraphOptimizationLevel, Session, SessionInputValue};
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

/// merged decoder 的注意力几何（来自模型 config.json：d_model 512、
/// decoder 8 头、6 层）。第一步要显式喂 24 个空 past 张量，形状必须对：
/// 6 层 × (self-attention k/v + cross-attention k/v) = 24。
const DEC_HEADS: i64 = 8;
const DEC_LAYERS: usize = 6;
const HEAD_DIM: i64 = 64;

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

/// 可选的加速版 decoder（`scripts/mt/transform_lmhead.py` 生成）。
///
/// 原版模型把词表投影写成 `Transpose(权重) -> DequantizeLinear -> MatMul`，
/// 而这一段在 merged decoder 的 `If` 子图里，ORT 的常量折叠管不到，
/// 于是**每生成一个 token 都要重新转置 33 MB 权重、再展开成 133 MB fp32**。
/// 实测单步 14.8 ms，其中约 12 ms 是这两步纯浪费。
///
/// 加速版把同样的权重换成正向转置好的 int8 副本，词表投影直接走
/// `DynamicQuantizeLinear + MatMulInteger`，只反量化 1×65001 的输出。
/// 单步降到 2.4 ms，argmax 输出与原版逐位一致。
///
/// 代价是文件大 32 MB（多一份 int8 转置权重），所以做成可选文件：
/// 有就用，没有就退回原版，下载流程完全不用改。
const FAST_DECODER: &str = "decoder.int8.onnx";

fn decoder_name(dir: &Path) -> &'static str {
    if file_ok(&dir.join(FAST_DECODER), 60 * 1024 * 1024) {
        FAST_DECODER
    } else {
        "decoder.onnx"
    }
}

/// 三个文件齐了才算可用。阈值给得比实际大小低一截，只防"下了一半"。
pub fn installed() -> bool {
    let d = models_dir();
    file_ok(&d.join("encoder.onnx"), 40 * 1024 * 1024)
        && file_ok(&d.join(decoder_name(&d)), 45 * 1024 * 1024)
        && file_ok(&d.join("tokens.tsv"), 1024 * 1024)
}

/// 给界面用的模型状态。
#[derive(serde::Serialize)]
pub struct ModelStatus {
    pub installed: bool,
    pub dir: String,
    pub bytes: u64,
    /// 实际生效的 decoder 文件名（`decoder.int8.onnx` 表示走了加速版）
    pub decoder: String,
}

pub fn status() -> ModelStatus {
    let d = models_dir();
    let dec = decoder_name(&d);
    let bytes = ["encoder.onnx", dec, "tokens.tsv"]
        .iter()
        .filter_map(|f| d.join(f).metadata().ok().map(|m| m.len()))
        .sum();
    ModelStatus {
        installed: installed(),
        dir: d.display().to_string(),
        bytes,
        decoder: dec.to_string(),
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
            let prev_cjk = prev.map(is_cjk).unwrap_or(false);
            let drop = match nxt {
                // 标点前不留空格
                Some(n) if ZH_PUNCT.contains(n) => true,
                // 半角标点紧跟中文时下面会转成全角，空格现在就得去掉，
                // 否则会留下"吗 ？"这种两侧带空的全角标点
                Some(n) if prev_cjk && ",?!;:".contains(n) => true,
                // 中文之间不留空格
                Some(n) if is_cjk(n) && prev_cjk => true,
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

    // 全角标点后面若还有空格，一并去掉（spm 可能连出两个）
    let mut cleaned = String::with_capacity(out.len());
    let mut it = out.chars().peekable();
    while let Some(c) = it.next() {
        cleaned.push(c);
        while ZH_PUNCT.contains(c) && it.peek() == Some(&' ') {
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
    /// decoder 的输入名，装载时按模型声明的顺序抓一次。
    /// merged decoder 的 past 名字里带层号，硬编码顺序换模型就会静默错位。
    dec_inputs: Vec<String>,
}

/// 一层注意力的 KV 缓存副本。
///
/// 为什么要复制：`Session::run` 的输出借用 session（`run(&mut self)`），
/// 同一 session 的下一轮输入不能直接引用上一轮的输出。24 个张量每步
/// 复制一次，代价是几百 KB 的 memcpy，比重新算一遍便宜两个数量级。
#[derive(Clone)]
struct Past {
    shape: Vec<i64>,
    data: Vec<f32>,
}

/// 解码耗时分解。只给 `translate_probe` 用——正常翻译路径不传，
/// 也就不为计时付任何代价。
#[derive(Default, Clone, Copy)]
pub struct Stats {
    /// encoder 一次前向
    pub enc_ms: u128,
    /// decoder 全部步数合计
    pub dec_ms: u128,
    /// 第一步耗时。KV cache 的收益主要体现在"不再随长度变慢"，
    /// 所以首步与后续步必须分开看
    pub first_step_ms: u128,
    /// 生成了多少个 token（不含 EOS）
    pub steps: usize,
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
    let dec = mk(decoder_name(&dir))?;

    // 输入名只在装载时取一次。merged decoder 有 27 个输入，其中 24 个是
    // past_key_values.N.{decoder,encoder}.{key,value}，靠名字分派比靠下标稳。
    let dec_inputs: Vec<String> = dec.inputs().iter().map(|o| o.name().to_string()).collect();
    let past = dec_inputs
        .iter()
        .filter(|n| n.starts_with("past_key_values."))
        .count();
    if past != DEC_LAYERS * 4 {
        return Err(format!(
            "decoder 不是带 KV cache 的合并版：past 张量 {past} 个，应为 {}。\
             旧版模型请重新下载离线模型。",
            DEC_LAYERS * 4
        ));
    }

    Ok(Model {
        enc,
        dec,
        vocab,
        dec_inputs,
    })
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
        with_model(|m| m.generate(&req.text, max_new, &mut Stats::default()))
    }
}

impl Model {
    /// 编码 → 逐步解码 → 后处理。计时段写进 `st`。
    fn generate(&mut self, text: &str, max_new: usize, st: &mut Stats) -> Result<String, String> {
        let ids = self.vocab.encode(text);
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

        let t_enc = std::time::Instant::now();
        let t_ids = Tensor::from_array((vec![1i64, len as i64], src))
            .map_err(|e| format!("构造 encoder 输入失败：{e}"))?;
        let t_mask = Tensor::from_array((vec![1i64, len as i64], mask.clone()))
            .map_err(|e| format!("构造 mask 失败：{e}"))?;

        // 输入顺序取自模型定义（encoder: input_ids, attention_mask），
        // 不能用名字传，rc.15 的 inputs! 只认位置。
        let enc_out = self
            .enc
            .run(ort::inputs![t_ids, t_mask])
            .map_err(|e| format!("encoder 推理失败：{e}"))?;
        let (h_shape, hidden) = enc_out[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("读取 encoder 输出失败：{e}"))?;
        let hidden: Vec<f32> = hidden.to_vec();
        // 用真实输出形状而不是硬编码 512，换模型时不会错位
        let h_dims: Vec<i64> = h_shape.iter().map(|&d| d as i64).collect();
        st.enc_ms = t_enc.elapsed().as_millis();

        // 这两份输入每一步都要喂，建一次、借用多次。旧实现每步都
        // clone 一遍 hidden，长句等于白拷 1 MB。
        let t_mask = Tensor::from_array((vec![1i64, len as i64], mask))
            .map_err(|e| format!("构造 decoder mask 失败：{e}"))?;
        let t_h = Tensor::from_array((h_dims, hidden))
            .map_err(|e| format!("构造 hidden 失败：{e}"))?;

        // past 名按模型声明顺序收集，下标与缓存一一对应
        let past_names: Vec<String> = self
            .dec_inputs
            .iter()
            .filter(|n| n.starts_with("past_key_values."))
            .cloned()
            .collect();
        // 第一步没有 past。merged decoder 的 24 个 past 输入不能省，而
        // ONNX Runtime 的 Reshape 不接受 0 长度维（"The dimension with
        // value zero exceeds..."），所以首步喂 seq=1 的零张量占位，
        // 同时把 use_cache_branch 置假——那一路根本不读 past。
        let mut cache: Vec<Past> = vec![
            Past {
                shape: vec![1, DEC_HEADS, 0, HEAD_DIM],
                data: Vec::new(),
            };
            past_names.len()
        ];

        let t_dec = std::time::Instant::now();
        let mut cur = PAD as i64;
        let mut out: Vec<u32> = Vec::with_capacity(max_new);

        for step in 0..max_new {
            let t_step = std::time::Instant::now();

            // 按名字喂而不是按位置拼数组：27 个输入里 24 个是 past，
            // 有序数组一旦错位，模型不会报错，只会安静地输出垃圾
            let mut feeds: Vec<(String, SessionInputValue)> =
                Vec::with_capacity(self.dec_inputs.len());
            for name in &self.dec_inputs {
                let v: SessionInputValue = match name.as_str() {
                    "input_ids" => SessionInputValue::from(
                        Tensor::from_array((vec![1i64, 1i64], vec![cur]))
                            .map_err(|e| format!("构造 decoder 输入失败：{e}"))?,
                    ),
                    "encoder_attention_mask" => SessionInputValue::from(t_mask.view()),
                    "encoder_hidden_states" => SessionInputValue::from(t_h.view()),
                    // merged 版靠这个布尔量在 If 节点里选分支：
                    // 真 → 算完新 token 后 present = concat(past, new)
                    // 假 → 完全忽略 past，重算整段
                    // 只有首步需要"假"：cross-attention 的 K/V 必须在这一步
                    // 由 encoder 输出算出来，之后才能走缓存。
                    "use_cache_branch" => SessionInputValue::from(
                        Tensor::from_array((vec![1i64], vec![step > 0]))
                            .map_err(|e| format!("构造 use_cache_branch 失败：{e}"))?,
                    ),
                    n => {
                        let i = past_names
                            .iter()
                            .position(|p| p == n)
                            .ok_or_else(|| format!("decoder 出现未知输入：{n}"))?;
                        let (shape, data) = (cache[i].shape.clone(), cache[i].data.clone());
                        SessionInputValue::from(
                            Tensor::from_array((shape, data))
                                .map_err(|e| format!("构造 {n} 失败：{e}"))?,
                        )
                    }
                };
                feeds.push((name.clone(), v));
            }

            let outs = self
                .dec
                .run(feeds)
                .map_err(|e| format!("decoder 第 {} 步推理失败：{e}", step + 1))?;

            // 有 cache 之后每步只算一个位置，输出就是 [1, 1, VOCAB]
            let logits = outs
                .get("logits")
                .ok_or_else(|| "decoder 没有 logits 输出".to_string())?;
            let (_, logits) = logits
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("读取 decoder 输出失败：{e}"))?;
            if logits.len() < VOCAB {
                return Err(format!(
                    "decoder 输出维度不符：len={} 期望 {}",
                    logits.len(),
                    VOCAB
                ));
            }
            let row = &logits[logits.len() - VOCAB..];

            // present.N.* → 下一轮的 past_key_values.N.*
            for (i, name) in past_names.iter().enumerate() {
                // cross-attention 的 K/V 整段解码都不变（encoder 输出是固定的），
                // 首步收下之后就再也不动。merged 导出在 cache 分支给出的
                // present.*.encoder.* 形状是坏的（batch 维变 0），照收下一步必崩。
                if step > 0 && name.contains(".encoder.") {
                    continue;
                }
                let out_name = name.replacen("past_key_values.", "present.", 1);
                let Some(v) = outs.get(&out_name) else {
                    continue;
                };
                let (shape, data) = v
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("读取 {out_name} 失败：{e}"))?;
                let c = &mut cache[i];
                c.shape = shape.iter().map(|&d| d as i64).collect();
                // 复用已有分配：每步 new 一个 Vec 会让分配器成为热点
                c.data.clear();
                c.data.extend_from_slice(data);
            }

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

            if step == 0 {
                st.first_step_ms = t_step.elapsed().as_millis();
            }

            if best_id == EOS {
                break;
            }
            cur = best_id as i64;
            out.push(best_id);
        }

        st.dec_ms = t_dec.elapsed().as_millis();
        st.steps = out.len();

        Ok(self.vocab.decode(&out))
    }
}

/// 只给基准测试用：跑一次离线翻译并回报各阶段耗时。
///
/// 界面上的"按下到出结果"混着 HTTP、分段、缓存和渲染，看不出 decoder
/// 本身快了多少；离线引擎的提速必须单独量。
pub fn translate_probe(text: &str, max_new: usize) -> Result<(String, Stats), String> {
    if !installed() {
        return Err("模型未下载".into());
    }
    let mut st = Stats::default();
    let t = with_model(|m| m.generate(text, max_new, &mut st))?;
    Ok((t, st))
}

/// 打印 decoder 的输入输出签名。换模型版本时先看这个再改代码，
/// 比按文档猜形状快——merged 版就多出一个 `use_cache_branch`。
pub fn dump_io() -> Result<String, String> {
    use ort::value::ValueType;
    use std::fmt::Write as _;

    with_model(|m| {
        let mut s = String::new();
        for o in m.dec.inputs() {
            match o.dtype() {
                ValueType::Tensor { shape, .. } => {
                    let _ = writeln!(s, "IN  {:<44} {shape}", o.name());
                }
                other => {
                    let _ = writeln!(s, "IN  {:<44} {other:?}", o.name());
                }
            }
        }
        for o in m.dec.outputs() {
            if let ValueType::Tensor { shape, .. } = o.dtype() {
                let _ = writeln!(s, "OUT {:<44} {shape}", o.name());
            }
        }
        Ok(s)
    })
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
///
/// decoder 必须是 **merged** 版：它把"首步无 cache"和"后续步带 cache"
/// 两条路径合进一个图，输入里带 24 个 past_key_values。普通
/// `decoder_model_quantized.onnx` 只有全序列重算一条路，每步都要
/// 把整段前缀重跑一遍（实测 ~20 ms/step，与长度线性相关）。
const FILES: &[(&str, &str, u64)] = &[
    (
        "encoder.onnx",
        "https://hf-mirror.com/Xenova/opus-mt-en-zh/resolve/main/onnx/encoder_model_quantized.onnx",
        52_899_742,
    ),
    (
        "decoder.onnx",
        "https://hf-mirror.com/Xenova/opus-mt-en-zh/resolve/main/onnx/decoder_model_merged_quantized.onnx",
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
        // 已经下好的不重下：换 decoder 版本时没必要重拉 52 MB 的 encoder
        if file_ok(&dir.join(name), total * 95 / 100) {
            overall += *total;
            continue;
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_drops_space_before_turned_fullwidth_punct() {
        // 模型常输出 "吗 ?"（半角问号前带空格）。若只做全角替换，
        // 会留下"吗 ？"——两侧都有空的全角标点
        assert_eq!(polish("关闭前保存更改吗 ?"), "关闭前保存更改吗？");
        assert_eq!(polish("连接失败 , 请重试"), "连接失败，请重试");
    }

    #[test]
    fn polish_keeps_latin_spacing() {
        // 含中文但夹着英文短语时，英文之间的空格不能删
        assert_eq!(polish("打开 settings 面板"), "打开 settings 面板");
        // 纯英文原样返回（只 trim）
        assert_eq!(polish("  Save changes?  "), "Save changes?");
    }

    #[test]
    fn polish_never_leaves_space_after_fullwidth_punct() {
        assert_eq!(polish("你好。 世界"), "你好。世界");
        assert_eq!(polish("完成！  继续"), "完成！继续");
    }
}
