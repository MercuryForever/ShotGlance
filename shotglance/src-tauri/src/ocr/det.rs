//! PP-OCRv4 文本检测（DB）前处理 + 后处理。
//!
//! 这里刻意不引入 opencv / imageproc：整条链路只需要"二值图找连通域"，
//! 一百多行手写比拉两个重量级依赖更划算，也更容易看清时间花在哪。
//!
//! 取舍说明：真正的 DB 后处理是「轮廓 → minAreaRect → unclip（可旋转框）」。
//! 这里做的是「连通域 → 轴对齐外接框 → 按面积/周长外扩」。差异只在
//! 倾斜排版的文本上明显（斜体标题、旋转水印），而截图→翻译的主要场景是
//! 网页/文档的正交排版，轴对齐足够，且省掉 minAreaRect 的凸包旋转卡壳。

use image::{imageops, RgbaImage};

/// 检测输入的最长边上限。越大越准、越慢：960 时 2560×1440 屏约 960×544。
pub const LIMIT_SIDE: u32 = 960;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// 概率图二值化阈值（PaddleOCR det_db_thresh）。
const BIN_THRESH: f32 = 0.3;
/// 框内平均置信度下限（det_db_box_thresh）。
const BOX_THRESH: f32 = 0.6;
/// 外扩比例（det_db_unclip_ratio）。1.5 是 Paddle 默认值。
const UNCLIP: f32 = 1.5;

/// 检测网络的输入张量，同时带回缩放比，供后续把框映射回原图坐标。
pub struct DetInput {
    /// NCHW 单样本，已归一化。
    pub data: Vec<f32>,
    pub w: u32,
    pub h: u32,
    /// 原图宽 / 输入宽。映射回原图时乘它。
    pub scale_x: f32,
    pub scale_y: f32,
}

/// 原图坐标系下的一个文本框。
#[derive(Clone, Debug)]
pub struct TextBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 缩放到检测输入尺寸并归一化为 NCHW。
///
/// 两个细节：
/// - 尺寸必须补齐到 32 的倍数，否则 DB 的 3 次下采样 + 上采样拼接会尺寸错位。
/// - 只缩小不放大（`min(1.0)`）：小截图放大后再检测不会变准，只变慢。
pub fn preprocess(img: &RgbaImage) -> DetInput {
    let (ow, oh) = img.dimensions();
    let max_side = ow.max(oh) as f32;
    let scale = (LIMIT_SIDE as f32 / max_side).min(1.0);

    let tw = ((((ow as f32 * scale).round().max(32.0)) as u32 + 31) / 32) * 32;
    let th = ((((oh as f32 * scale).round().max(32.0)) as u32 + 31) / 32) * 32;

    let resized = imageops::resize(img, tw, th, imageops::FilterType::Triangle);

    let plane = (tw * th) as usize;
    let mut data = vec![0f32; plane * 3];
    for (i, px) in resized.pixels().enumerate() {
        // RGBA → RGB，按通道拆成三段连续内存（NCHW 的 C 维在外）。
        for c in 0..3 {
            let v = px.0[c] as f32 / 255.0;
            data[c * plane + i] = (v - MEAN[c]) / STD[c];
        }
    }

    DetInput {
        data,
        w: tw,
        h: th,
        scale_x: ow as f32 / tw as f32,
        scale_y: oh as f32 / th as f32,
    }
}

/// DB 概率图 → 文本框。
///
/// 步骤：二值化 → 8 邻域连通域 → 面积/均值过滤 → 按 unclip 外扩 → 映射回原图。
pub fn postprocess(
    prob: &[f32],
    pw: u32,
    ph: u32,
    scale_x: f32,
    scale_y: f32,
) -> Vec<TextBox> {
    let (pw, ph) = (pw as usize, ph as usize);
    let expected = pw * ph;
    if prob.len() < expected {
        return Vec::new();
    }

    let mut visited = vec![false; expected];
    let mut boxes = Vec::new();
    let mut stack: Vec<usize> = Vec::with_capacity(256);

    for start in 0..expected {
        if visited[start] || prob[start] <= BIN_THRESH {
            continue;
        }

        // 迭代式 DFS。用显式栈而不是递归：一条长文本行能连通上万个像素，
        // 递归深度会把栈打穿。
        stack.clear();
        stack.push(start);
        visited[start] = true;

        let (mut min_x, mut max_x, mut min_y, mut max_y) = (pw, 0usize, ph, 0usize);
        let mut sum = 0f32;
        let mut count = 0usize;

        while let Some(idx) = stack.pop() {
            let (x, y) = (idx % pw, idx / pw);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            sum += prob[idx];
            count += 1;

            // 8 邻域：DB 输出边缘是渐变的，4 邻域容易把一条行切成几段。
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(pw - 1);
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(ph - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let n = ny * pw + nx;
                    if !visited[n] && prob[n] > BIN_THRESH {
                        visited[n] = true;
                        stack.push(n);
                    }
                }
            }
        }

        if count < 12 {
            continue; // 噪点
        }
        // 框内平均概率过低的一律丢弃：DB 的 sigmoid 输出在背景上也会有
        // 0.3–0.5 的残值，只看二值化会把大片背景当文本框。
        if sum / (count as f32) < BOX_THRESH {
            continue;
        }

        let bw = (max_x - min_x + 1) as f32;
        let bh = (max_y - min_y + 1) as f32;
        if bw < 3.0 || bh < 3.0 {
            continue;
        }

        // unclip 的近似：offset = area * ratio / perimeter。
        // 对细长文本行（如 200×12）约等于 8px，正好把 DB 收缩掉的边界补回来；
        // 对正方形区域几乎为 0，不会把相邻两行糊在一起。
        let offset = (bw * bh * UNCLIP) / (2.0 * (bw + bh)).max(1.0);

        let ox0 = ((min_x as f32 - offset) * scale_x).max(0.0);
        let oy0 = ((min_y as f32 - offset) * scale_y).max(0.0);
        let ox1 = ((max_x as f32 + 1.0 + offset) * scale_x).max(0.0);
        let oy1 = ((max_y as f32 + 1.0 + offset) * scale_y).max(0.0);

        let (x, y) = (ox0 as u32, oy0 as u32);
        let (w, h) = ((ox1 - ox0) as u32, (oy1 - oy0) as u32);
        if w == 0 || h == 0 {
            continue;
        }
        boxes.push(TextBox { x, y, w, h });
    }

    reading_order(boxes)
}

/// 按阅读顺序排序：先分行（纵向中心相近的归为同一行），行内从左到右。
///
/// 直接按 (y, x) 排序在标题字号明显大于正文时会错行——大字的 y 从更上方开始，
/// 排序后会被插到上一行中间。用"中心线聚合"更稳。
fn reading_order(mut boxes: Vec<TextBox>) -> Vec<TextBox> {
    if boxes.len() <= 1 {
        return boxes;
    }
    boxes.sort_by_key(|b| b.y + b.h / 2);

    let median_h = {
        let mut hs: Vec<u32> = boxes.iter().map(|b| b.h).collect();
        hs.sort_unstable();
        hs[hs.len() / 2]
    };
    // 容差取半行高：字号差异 2 倍以内仍算同一行。
    let tol = (median_h / 2).max(4);

    let mut out: Vec<TextBox> = Vec::with_capacity(boxes.len());
    let mut row: Vec<TextBox> = Vec::new();
    let mut row_y = 0u32;

    let flush = |row: &mut Vec<TextBox>, out: &mut Vec<TextBox>| {
        row.sort_by_key(|b| b.x);
        out.append(row);
    };

    for b in boxes {
        let cy = b.y + b.h / 2;
        if row.is_empty() {
            row_y = cy;
            row.push(b);
        } else if cy.abs_diff(row_y) <= tol {
            row.push(b);
        } else {
            flush(&mut row, &mut out);
            row_y = cy;
            row.push(b);
        }
    }
    flush(&mut row, &mut out);

    out
}

/// 裁一个框（带少量内缩），用于送进识别网络。
///
/// 内缩 1px 是为了切掉 unclip 外扩带进来的相邻像素——那些像素在
/// 识别网络里会成为噪声，尤其影响行首行尾的标点。
pub fn crop(img: &RgbaImage, b: &TextBox) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let pad = 1u32;
    let x = (b.x + pad).min(iw.saturating_sub(1));
    let y = (b.y + pad).min(ih.saturating_sub(1));
    let w = b.w.saturating_sub(pad * 2).max(1).min(iw - x);
    let h = b.h.saturating_sub(pad * 2).max(1).min(ih - y);
    imageops::crop_imm(img, x, y, w, h).to_image()
}
