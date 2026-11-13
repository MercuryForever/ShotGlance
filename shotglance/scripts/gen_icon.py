"""生成 ShotGlance 图标：纯 stdlib，无 PIL 依赖。

渲染 1056x1056（SDF + 1px AA），再逐级降采样到 512/256/128/32。
输出: icon.png(512), 32x32.png, 128x128.png, icon.ico(256)
"""
import math
import os
import struct
import zlib

W = 1056
BG = (0x18, 0x5F, 0xA5)
FG = (0xFF, 0xFF, 0xFF)


def sdf_round_rect(px, py, cx, cy, hw, hh, r):
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    ax, ay = max(qx, 0.0), max(qy, 0.0)
    return math.hypot(ax, ay) + min(max(qx, qy), 0.0) - r


def sdf_segment(px, py, x1, y1, x2, y2):
    dx, dy = x2 - x1, y2 - y1
    length2 = dx * dx + dy * dy
    t = 0.0 if length2 == 0 else max(0.0, min(1.0, ((px - x1) * dx + (py - y1) * dy) / length2))
    return math.hypot(px - (x1 + t * dx), py - (y1 + t * dy))


def cov(d):
    return min(1.0, max(0.0, 0.5 - d))


def render():
    buf = bytearray(W * W * 4)
    s = W / 1056.0
    pad = 26 * s
    rad = 210 * s
    # 放大镜
    lx, ly, lr, lw = 452 * s, 470 * s, 250 * s, 62 * s
    hx1, hy1, hx2, hy2, hr = 640 * s, 658 * s, 812 * s, 830 * s, 46 * s
    # 镜片内三条“文字行”
    lines = [
        (330 * s, 400 * s, 570 * s, 400 * s, 20 * s),
        (330 * s, 470 * s, 520 * s, 470 * s, 20 * s),
        (330 * s, 540 * s, 590 * s, 540 * s, 20 * s),
    ]
    bg_cx = bg_cy = W / 2.0
    bg_hw = bg_hh = W / 2.0 - pad
    bg_r = rad

    for y in range(W):
        py = y + 0.5
        row = y * W * 4
        for x in range(W):
            px = x + 0.5
            a = cov(sdf_round_rect(px, py, bg_cx, bg_cy, bg_hw, bg_hh, bg_r))
            if a <= 0.0:
                continue
            r, g, b = BG

            f = cov(abs(math.hypot(px - lx, py - ly) - lr) - lw / 2.0)
            if f <= 0.0:
                f = cov(sdf_segment(px, py, hx1, hy1, hx2, hy2) - hr)
            if f <= 0.0:
                for x1, y1, x2, y2, t in lines:
                    f = cov(sdf_segment(px, py, x1, y1, x2, y2) - t)
                    if f > 0.0:
                        break
            if f > 0.0:
                r = int(r + (FG[0] - r) * f)
                g = int(g + (FG[1] - g) * f)
                b = int(b + (FG[2] - b) * f)

            i = row + x * 4
            buf[i] = r
            buf[i + 1] = g
            buf[i + 2] = b
            buf[i + 3] = int(255 * a)
    return buf


def downsample(buf, src, dst):
    factor = src // dst
    out = bytearray(dst * dst * 4)
    area = factor * factor
    for y in range(dst):
        for x in range(dst):
            sr = sg = sb = sa = 0
            for dy in range(factor):
                base = ((y * factor + dy) * src + x * factor) * 4
                for dx in range(factor):
                    i = base + dx * 4
                    a = buf[i + 3]
                    sr += buf[i] * a
                    sg += buf[i + 1] * a
                    sb += buf[i + 2] * a
                    sa += a
            o = (y * dst + x) * 4
            if sa:
                out[o] = sr // sa
                out[o + 1] = sg // sa
                out[o + 2] = sb // sa
            out[o + 3] = sa // area
    return out


def png_bytes(w, buf):
    raw = bytearray()
    stride = w * 4
    for y in range(w):
        raw.append(0)
        raw += buf[y * stride:(y + 1) * stride]

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, w, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def main():
    out = os.path.join(os.path.dirname(__file__), "..", "shotglance", "src-tauri", "icons")
    os.makedirs(out, exist_ok=True)
    big = render()
    sizes = {512: None, 256: None, 128: None, 32: None}
    for s in sizes:
        sizes[s] = downsample(big, W, s)

    with open(os.path.join(out, "icon.png"), "wb") as f:
        f.write(png_bytes(512, sizes[512]))
    with open(os.path.join(out, "128x128.png"), "wb") as f:
        f.write(png_bytes(128, sizes[128]))
    with open(os.path.join(out, "32x32.png"), "wb") as f:
        f.write(png_bytes(32, sizes[32]))

    ico_png = png_bytes(256, sizes[256])
    header = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(ico_png), 22)
    with open(os.path.join(out, "icon.ico"), "wb") as f:
        f.write(header + entry + ico_png)

    for name in ("icon.png", "128x128.png", "32x32.png", "icon.ico"):
        p = os.path.join(out, name)
        print(f"{name:14s} {os.path.getsize(p):>7d} bytes")


if __name__ == "__main__":
    main()
