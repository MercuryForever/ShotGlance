"""解码结构 A/B：全序列重算 vs merged decoder + KV cache（构建期一次性）。

为什么要专门量：界面上的"按下到出结果"混着分段、缓存和渲染，看不出
decoder 本身快多少；而且这类改动的收益**强依赖输出长度**，只报一个
"快了 N 倍"是没意义的。

用法：
  python bench_decode.py <decoder_model_quantized.onnx> <decoder_model_merged_quantized.onnx>

两个模型都在 `$APPDATA/ShotGlance/models/opus-mt-en-zh/` 下（旧版文件名是
decoder.onnx，换过 merged 之后旧文件的副本要自己留）。

关键手法：生成 token 固定成 `100 + step % 50`，不能用 argmax——真实模型
几步就会吐 EOS，那样量不到 n=50/80 的点，曲线永远是平的。
"""

import sys
import time

import numpy as np
import onnxruntime as ort

PAD, EOS = 65000, 0
B, H, HD, L = 1, 8, 64, 5   # d_model 512 / 8 头；L 是假的 encoder 长度，只影响耗时

OLD = sys.argv[1] if len(sys.argv) > 1 else "decoder_model_quantized.onnx"
NEW = sys.argv[2] if len(sys.argv) > 2 else "decoder_model_merged_quantized.onnx"

so = ort.SessionOptions()
so.intra_op_num_threads = 4
mask = np.ones((1, L), np.int64)
hidden = np.zeros((1, L, 512), np.float32)

old = ort.InferenceSession(OLD, sess_options=so, providers=["CPUExecutionProvider"])
new = ort.InferenceSession(NEW, sess_options=so, providers=["CPUExecutionProvider"])
new_names = [o.name for o in new.get_outputs()]


def full_seq(n):
    """每步把整段前缀喂进去重算。"""
    cur = [PAD]
    t0 = time.perf_counter()
    for step in range(n):
        old.run(None, {"input_ids": np.array([cur], np.int64),
                       "encoder_attention_mask": mask,
                       "encoder_hidden_states": hidden})
        cur.append(100 + step % 50)
    return (time.perf_counter() - t0) * 1000


def kv_cache(n):
    """每步只喂 1 个 token，历史放 past_key_values。"""
    past = {f"past_key_values.{l}.{k}.{v}": np.zeros((B, H, 0, HD), np.float32)
            for l in range(6) for k in ("decoder", "encoder") for v in ("key", "value")}
    tok = PAD
    t0 = time.perf_counter()
    for step in range(n):
        f = {"encoder_attention_mask": mask, "encoder_hidden_states": hidden,
             "input_ids": np.array([[tok]], np.int64),
             "use_cache_branch": np.array([step > 0])}
        f.update(past)
        d = dict(zip(new_names, new.run(None, f)))
        # cross-attention 的 present 在 cache 分支里形状是坏的（batch 变 0），
        # encoder 侧 K/V 又全程不变，所以只收首步
        for k, v in d.items():
            if k.startswith("present.") and not (step > 0 and ".encoder." in k):
                past[k.replace("present.", "past_key_values.")] = v
        tok = 100 + step % 50
    return (time.perf_counter() - t0) * 1000


def best(fn, n, rounds=3):
    fn(3)  # 预热
    return min(fn(n) for _ in range(rounds))


print(f"{'tokens':>6} {'full-seq':>10} {'kv-cache':>10} {'speedup':>8}")
for n in (8, 13, 26, 50, 80):
    a, b = best(full_seq, n), best(kv_cache, n)
    print(f"{n:>6} {a:>9.0f}ms {b:>9.0f}ms {a / b:>7.2f}x")
