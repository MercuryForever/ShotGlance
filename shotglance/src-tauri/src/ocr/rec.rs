//! PP-OCRv4 文本识别（CRNN + CTC）前处理与解码。

use image::{imageops, RgbaImage};

/// 识别网络固定输入高。PP-OCR rec 全系列都是 48。
pub const INPUT_H: u32 = 48;
/// 单行最长输入宽。超长行（整段代码）压缩到这个宽度后仍能读，但再宽只增加时延。
pub const MAX_W: u32 = 960;
/// 最窄输入宽，避免极短词（"OK"）产生 8px 宽的退化张量。
pub const MIN_W: u32 = 32;

/// 缩放到高 48、按比例定宽，并归一化到 [-1, 1]（(x/255-0.5)/0.5）。
///
/// 不足宽度的部分补 0（等价于中灰），与 PaddleOCR 保持一致。
pub fn preprocess(crop: &RgbaImage, width: u32) -> Vec<f32> {
    let (cw, ch) = crop.dimensions();
    if cw == 0 || ch == 0 {
        return vec![0f32; (3 * width * INPUT_H) as usize];
    }

    let ratio = cw as f32 / ch as f32;
    let target_w = ((INPUT_H as f32 * ratio).round() as u32).clamp(MIN_W, MAX_W);
    let resized = imageops::resize(crop, target_w, INPUT_H, imageops::FilterType::Triangle);

    let plane = (width * INPUT_H) as usize;
    let mut data = vec![0f32; plane * 3];
    for (i, px) in resized.pixels().enumerate() {
        let x = (i as u32) % target_w;
        if x >= width {
            continue; // 调用方给的张量宽度小于内容宽度时防越界
        }
        let y = (i as u32) / target_w;
        let idx = (y * width + x) as usize;
        for c in 0..3 {
            data[c * plane + idx] = (px.0[c] as f32 / 255.0 - 0.5) / 0.5;
        }
    }
    data
}

/// 按行宽推算该用多宽的输入张量（动态宽度模型用）。
pub fn width_for(crop: &RgbaImage) -> u32 {
    let (cw, ch) = crop.dimensions();
    if cw == 0 || ch == 0 {
        return MIN_W;
    }
    let ratio = cw as f32 / ch as f32;
    (((INPUT_H as f32 * ratio).round() as u32).clamp(MIN_W, MAX_W) + 15) / 16 * 16
}

/// CTC 贪心解码的结果。
pub struct RecOutput {
    pub text: String,
    pub conf: f32,
}

/// CTC 贪心解码 + 置信度。
///
/// 规则就是标准 CTC:每个时间步取 argmax,跳过 blank(0),再合并相邻重复。
/// 置信度取保留字符的概率均值——比取 max 更贴近"这行读得准不准",
/// 因为误识别通常只发生在个别字上。
pub fn decode(logits: &[f32], steps: usize, classes: usize, keys: &[String]) -> RecOutput {
    let mut text = String::new();
    let mut conf_sum = 0f32;
    let mut kept = 0usize;
    let mut prev = usize::MAX;

    for t in 0..steps {
        let base = t * classes;
        if base + classes > logits.len() {
            break;
        }
        let row = &logits[base..base + classes];

        // 手写 argmax：classes=6625，用 iterator 的 max_by 每次比较都要
        // 搬一趟 Option，on 640 个时间步上就是 400 万次分支，肉眼可见地慢。
        let mut best = 0usize;
        let mut best_v = row[0];
        for (i, &v) in row.iter().enumerate().skip(1) {
            if v > best_v {
                best_v = v;
                best = i;
            }
        }

        if best != 0 && best != prev {
            if let Some(ch) = char_at(best, keys) {
                text.push_str(ch);
                conf_sum += best_v;
                kept += 1;
            }
        }
        prev = best;
    }

    RecOutput {
        text,
        conf: if kept == 0 { 0.0 } else { conf_sum / kept as f32 },
    }
}

/// 索引 0 是 CTC blank，1..=n 是字典字符，最后一个是空格。
///
/// PaddleOCR 的约定：`character = ['blank'] + dict + [' ']`。
/// 字典文件本身不含空格行，所以空格必须挂在末尾，少这一条会让
/// 英文句子所有词粘成一串。
fn char_at(idx: usize, keys: &[String]) -> Option<&str> {
    if idx >= 1 && idx <= keys.len() {
        Some(keys[idx - 1].as_str())
    } else if idx == keys.len() + 1 {
        Some(" ")
    } else {
        None
    }
}
