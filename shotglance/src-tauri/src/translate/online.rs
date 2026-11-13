//! 在线引擎。
//!
//! 四家实现放在同一个文件里，因为它们共享的东西太多（语言码映射、
//! 响应取文本、错误包装），拆开反而要反复 import。
//!
//! 免密钥的两家（MyMemory / Google）放在前面：开箱可用这一点对
//! "装完就能翻"的体验是决定性的，用户不该为了试一下功能先去申请 key。

use serde_json::Value;

use super::{agent, describe_http_err, Cfg, Engine, Request};

/// 各家的语言码体系不一样，统一在这里转。
/// 传 `auto` 时源语言交给引擎自己判断（Google 支持，其他家退化成猜测）。
fn map_code(engine: &str, code: &str, is_target: bool) -> String {
    match engine {
        // DeepL 的语言码全大写，且目标语言的英语要带地区后缀
        "deepl" => match code {
            "auto" => "auto".into(),
            "zh-CN" | "zh" => "ZH".into(),
            "en" if is_target => "EN-US".into(),
            other => other.to_uppercase(),
        },
        // MyMemory 用 zh-CN 但源语言必须具体，没有 auto
        "mymemory" => match code {
            "auto" => "auto".into(),
            "zh" => "zh-CN".into(),
            other => other.to_string(),
        },
        // Google 直接用 BCP-47，几乎不用转
        _ => match code {
            "zh" => "zh-CN".into(),
            other => other.to_string(),
        },
    }
}

/// 源语言 `auto` 时给不支持自动检测的引擎兜底。
/// 判据是 CJK 字符占比——OCR 场景只有中英两种可能，够用了。
fn guess_source(text: &str) -> &'static str {
    let total = text.chars().count().max(1);
    let cjk = text
        .chars()
        .filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c))
        .count();
    if cjk * 10 >= total * 2 {
        "zh"
    } else {
        "en"
    }
}

fn resolve_source(engine: &str, req: &Request) -> String {
    if req.source == "auto" {
        match engine {
            "google" => "auto".to_string(),
            "deepl" => "auto".to_string(),
            _ => guess_source(&req.text).to_string(),
        }
    } else {
        req.source.clone()
    }
}

/* ---------------- MyMemory ---------------- */

/// MyMemory：完全免费、免密钥，匿名每天 5000 字符，带 email 提到 50000。
/// 质量一般，但它是"没有配置也没有网络条件时的最后一道在线兜底"。
pub struct MyMemory;

impl Engine for MyMemory {
    fn id(&self) -> &'static str {
        "mymemory"
    }
    fn label(&self) -> &'static str {
        "MyMemory（免费·免密钥）"
    }
    fn kind(&self) -> &'static str {
        "online"
    }

    fn ready(&self, _cfg: &Cfg) -> Result<(), String> {
        Ok(())
    }

    fn translate(&self, cfg: &Cfg, req: &Request) -> Result<String, String> {
        let src = resolve_source("mymemory", req);
        let tgt = map_code("mymemory", &req.target, true);
        let pair = format!("{src}|{tgt}");

        let mut call = agent()
            .get("https://api.mymemory.translated.net/get")
            .query("q", &req.text)
            .query("langpair", &pair);
        if !cfg.mymemory_email.trim().is_empty() {
            call = call.query("de", cfg.mymemory_email.trim());
        }

        let v: Value = call.call().map_err(|e| describe_http_err(e))?.into_json()
            .map_err(|e| format!("响应解析失败：{e}"))?;

        // 配额超限时 HTTP 仍是 200，错误藏在 responseStatus 里
        let status = v.get("responseStatus").and_then(|x| x.as_u64()).unwrap_or(0);
        if status != 200 {
            let detail = v
                .get("responseDetails")
                .and_then(|x| x.as_str())
                .unwrap_or("未知错误");
            return Err(format!("MyMemory {status}：{detail}"));
        }

        v.pointer("/responseData/translatedText")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "响应里没有 translatedText".to_string())
    }
}

/* ---------------- Google ---------------- */

/// Google 网页版的 gtx 端点。免密钥、质量最好、速度最快，
/// 但它是非公开接口，随时可能失效或因网络原因连不上——
/// 所以 `auto` 的降级链必须留后手。
pub struct Google;

impl Engine for Google {
    fn id(&self) -> &'static str {
        "google"
    }
    fn label(&self) -> &'static str {
        "Google 翻译（免密钥）"
    }
    fn kind(&self) -> &'static str {
        "online"
    }

    fn ready(&self, _cfg: &Cfg) -> Result<(), String> {
        Ok(())
    }

    fn translate(&self, _cfg: &Cfg, req: &Request) -> Result<String, String> {
        let src = resolve_source("google", req);
        let tgt = map_code("google", &req.target, true);

        let v: Value = agent()
            .get("https://translate.googleapis.com/translate_a/single")
            .query("client", "gtx")
            .query("sl", &src)
            .query("tl", &tgt)
            .query("dt", "t")
            .query("q", &req.text)
            .call()
            .map_err(|e| describe_http_err(e))?
            .into_json()
            .map_err(|e| format!("响应解析失败：{e}"))?;

        // 结构是 [[[译文, 原文, ...], ...], ...]，只取每段的第一个字段
        let segs = v
            .get(0)
            .and_then(|x| x.as_array())
            .ok_or_else(|| "响应结构不符合预期".to_string())?;

        let mut out = String::new();
        for s in segs {
            if let Some(t) = s.get(0).and_then(|x| x.as_str()) {
                out.push_str(t);
            }
        }
        if out.trim().is_empty() {
            return Err("Google 返回空译文".into());
        }
        Ok(out)
    }
}

/* ---------------- DeepL ---------------- */

/// DeepL：质量最好的一档，需要密钥。free 与 pro 是两个不同的域名，
/// 用错域名会直接 403——这是接入时最常见的坑，所以做成开关而不是让
/// 用户自己去改 URL。
pub struct DeepL;

impl Engine for DeepL {
    fn id(&self) -> &'static str {
        "deepl"
    }
    fn label(&self) -> &'static str {
        "DeepL（需密钥）"
    }
    fn kind(&self) -> &'static str {
        "online"
    }
    fn needs_config(&self) -> bool {
        true
    }

    fn ready(&self, cfg: &Cfg) -> Result<(), String> {
        if cfg.deepl_key.trim().is_empty() {
            Err("未填 API Key".into())
        } else {
            Ok(())
        }
    }

    fn translate(&self, cfg: &Cfg, req: &Request) -> Result<String, String> {
        let host = if cfg.deepl_pro {
            "https://api.deepl.com"
        } else {
            "https://api-free.deepl.com"
        };
        let url = format!("{host}/v2/translate");

        let src = resolve_source("deepl", req);
        // 先把所有值算成 owned，再一次性取 &str：直接往 vec 里塞
        // `&map_code(...)` 会让临时 String 在语句结束就析构，借不出来。
        let mut form: Vec<(String, String)> = vec![
            ("text".to_string(), req.text.clone()),
            ("target_lang".to_string(), map_code("deepl", &req.target, true)),
        ];
        // DeepL 明确拒绝 source_lang=auto，不传就是自动检测
        if src != "auto" {
            form.push(("source_lang".to_string(), map_code("deepl", &src, false)));
        }
        let pairs: Vec<(&str, &str)> = form
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let v: Value = agent()
            .post(&url)
            .set("Authorization", &format!("DeepL-Auth-Key {}", cfg.deepl_key.trim()))
            .send_form(&pairs)
            .map_err(|e| describe_http_err(e))?
            .into_json()
            .map_err(|e| format!("响应解析失败：{e}"))?;

        let mut out = String::new();
        if let Some(list) = v.get("translations").and_then(|x| x.as_array()) {
            for t in list {
                if let Some(s) = t.get("text").and_then(|x| x.as_str()) {
                    out.push_str(s);
                }
            }
        }
        if out.trim().is_empty() {
            return Err("DeepL 返回空译文".into());
        }
        Ok(out)
    }
}

/* ---------------- OpenAI 兼容 ---------------- */

/// 任何 OpenAI 兼容的 chat/completions 端点都能接：
/// OpenAI 官方、DeepSeek、通义、one-api 中转、本地 Ollama。
/// 这一档的价值在于"用户手里有什么就用什么"，而不是锁定某一家。
pub struct OpenAi;

impl Engine for OpenAi {
    fn id(&self) -> &'static str {
        "openai"
    }
    fn label(&self) -> &'static str {
        "OpenAI 兼容（需端点+密钥）"
    }
    fn kind(&self) -> &'static str {
        "online"
    }
    fn needs_config(&self) -> bool {
        true
    }

    fn ready(&self, cfg: &Cfg) -> Result<(), String> {
        if cfg.openai_base.trim().is_empty() {
            return Err("未填端点地址".into());
        }
        if cfg.openai_key.trim().is_empty() {
            return Err("未填 API Key".into());
        }
        Ok(())
    }

    fn translate(&self, cfg: &Cfg, req: &Request) -> Result<String, String> {
        let base = cfg.openai_base.trim().trim_end_matches('/');
        let url = format!("{base}/chat/completions");

        let tgt_name = match req.target.as_str() {
            "zh-CN" | "zh" => "简体中文",
            "en" => "English",
            "ja" => "日本語",
            other => other,
        };
        let src_hint = if req.source == "auto" {
            String::new()
        } else {
            format!("源语言是 {}。", req.source)
        };

        // 提示词刻意写死"只输出译文"：OCR 文本本身就有噪声，
        // 模型一旦加上"以下是翻译："这类前缀，界面里就没法用了。
        let sys = format!(
            "你是专业的翻译引擎。把用户给的文本翻译成{tgt_name}。{src_hint}\
             只输出译文本身，不要解释、不要加引号、不要复述原文。\
             保留原有的换行结构。文本可能来自截图 OCR，含识别错误时按最合理的意思翻译。"
        );

        let body = serde_json::json!({
            "model": cfg.openai_model.trim(),
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": sys },
                { "role": "user", "content": req.text },
            ],
        });

        let v: Value = agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {}", cfg.openai_key.trim()))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| describe_http_err(e))?
            .into_json()
            .map_err(|e| format!("响应解析失败：{e}"))?;

        v.pointer("/choices/0/message/content")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "响应里没有译文".to_string())
    }
}
