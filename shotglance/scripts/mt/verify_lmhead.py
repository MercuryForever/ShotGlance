"""对比原 decoder 与改造后的 decoder：数值一致性 + 单步耗时。

用法：
    python verify_lmhead.py <原模型> <新模型> [--steps N]

检查三项：
  1. 同一步的 logits 最大绝对误差（应 < 0.05，量化激活带来的误差）
  2. 每一步 argmax 是否一致（决定译文是否逐字相同）
  3. 单步耗时对比
"""

import argparse
import os
import time

import numpy as np
import onnxruntime as ort

HEADS, HEAD_DIM, LAYERS = 8, 64, 6
PAD = 65000


def make_session(path, threads=4):
    so = ort.SessionOptions()
    so.intra_op_num_threads = threads
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    return ort.InferenceSession(path, so, providers=["CPUExecutionProvider"])


def empty_past():
    return {
        f"past_key_values.{l}.{k}.{v}": np.zeros((1, HEADS, 0, HEAD_DIM), np.float32)
        for l in range(LAYERS)
        for k in ("decoder", "encoder")
        for v in ("key", "value")
    }


def run_steps(sess, hidden, mask, n_steps, capture_logits=None):
    """跑 n_steps 步贪婪解码，返回 (token 列表, 耗时 ms, 首步 logits)。"""
    names = [o.name for o in sess.get_outputs()]
    past = empty_past()
    tok = PAD
    tokens = []
    first_logits = None
    t0 = time.perf_counter()
    for step in range(n_steps):
        f = dict(past)
        f["encoder_attention_mask"] = mask
        f["input_ids"] = np.array([[tok]], np.int64)
        f["encoder_hidden_states"] = hidden
        f["use_cache_branch"] = np.array([step > 0])
        outs = dict(zip(names, sess.run(None, f)))
        for k, v in outs.items():
            if k.startswith("present.") and not (step > 0 and ".encoder." in k):
                past[k.replace("present.", "past_key_values.")] = v
        logits = outs["logits"]
        if step == 0:
            first_logits = logits[0, -1].copy()
        tok = int(logits[0, -1].argmax())
        tokens.append(tok)
    return tokens, (time.perf_counter() - t0) * 1000, first_logits


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--steps", type=int, default=20)
    ap.add_argument("--threads", type=int, default=4)
    args = ap.parse_args()

    enc_path = os.path.join(os.path.dirname(args.old), "encoder.onnx")
    enc = make_session(enc_path, args.threads)
    src = [104, 20, 2869, 209, 32, 26, 8, 0]
    L = len(src)
    feeds = {
        "input_ids": np.array([src], np.int64),
        "attention_mask": np.ones((1, L), np.int64),
    }
    enc.run(None, feeds)  # 预热
    hidden = np.ascontiguousarray(enc.run(None, feeds)[0], dtype=np.float32)
    mask = np.ones((1, L), np.int64)

    print(f"旧模型：{os.path.basename(args.old)}  {os.path.getsize(args.old) / 1048576:.1f} MB")
    print(f"新模型：{os.path.basename(args.new)}  {os.path.getsize(args.new) / 1048576:.1f} MB")
    print(f"步数：{args.steps}   线程：{args.threads}\n")

    old_s = make_session(args.old, args.threads)
    new_s = make_session(args.new, args.threads)

    # 预热（各自的首次 run 含内存分配，不计入）
    run_steps(old_s, hidden, mask, 3)
    run_steps(new_s, hidden, mask, 3)

    # ---------- 数值一致性 ----------
    print("=== 数值一致性 ===")
    ot, _, ol = run_steps(old_s, hidden, mask, args.steps)
    nt, _, nl = run_steps(new_s, hidden, mask, args.steps)
    diff = np.abs(ol - nl)
    print(f"  首步 logits 最大绝对误差 : {diff.max():.5f}")
    print(f"  首步 logits 平均绝对误差 : {diff.mean():.5f}")
    print(f"  首步 logits 值域         : [{ol.min():.2f}, {ol.max():.2f}]")
    top_old = int(ol.argmax())
    top_new = int(nl.argmax())
    print(f"  首步 argmax              : 旧={top_old} 新={top_new} "
          f"{'一致' if top_old == top_new else '★不一致'}")
    same = sum(a == b for a, b in zip(ot, nt))
    print(f"  序列 argmax 一致         : {same}/{len(ot)}")
    if same != len(ot):
        first_diff = next(i for i, (a, b) in enumerate(zip(ot, nt)) if a != b)
        print(f"    首次分歧在第 {first_diff} 步：旧={ot[first_diff]} 新={nt[first_diff]}")
    print(f"  旧 token  : {ot[:12]}")
    print(f"  新 token  : {nt[:12]}")

    # ---------- 性能 ----------
    print("\n=== 单步耗时（n=3 轮取最小）===")
    for label, sess in (("旧", old_s), ("新", new_s)):
        best = min(run_steps(sess, hidden, mask, args.steps)[1] for _ in range(3))
        print(f"  {label}：{best:7.0f} ms  ({best / args.steps:.2f} ms/步)")

    # ---------- 长序列（KV cache 才有优势的区间）----------
    print("\n=== 长序列 n=60（模拟长句翻译）===")
    for label, sess in (("旧", old_s), ("新", new_s)):
        best = min(run_steps(sess, hidden, mask, 60)[1] for _ in range(2))
        print(f"  {label}：{best:7.0f} ms  ({best / 60:.2f} ms/步)")


if __name__ == "__main__":
    main()
