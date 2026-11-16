"""算子级 profile：把 encoder / decoder 每个 ONNX 节点的耗时拆开。

用法：
    python profile_ops.py                      # 用默认模型目录
    python profile_ops.py <encoder.onnx> <decoder.onnx> [--steps N] [--threads T]

输出两部分：
  1. 按算子类型（op_name）聚合的耗时占比 —— 找瓶颈用
  2. 按单节点聚合的耗时 Top-N —— 找具体是哪一层/哪一块

注意：enable_profiling 本身有开销（每次 run 都要序列化事件），
所以拿到的**绝对毫秒会偏高**，但各算子之间的**相对占比**是可信的。
真实绝对值以 bench_decode.py 为准。
"""

import argparse
import collections
import json
import os
import sys
import tempfile

import numpy as np
import onnxruntime as ort

D_MODEL = 512
HEADS = 8
HEAD_DIM = 64
LAYERS = 6
VOCAB = 65001
PAD = 65000
DEC_HEADS = HEADS
DEC_HEAD_DIM = HEAD_DIM

# 一句有代表性的截图文本（会真正生成 10+ 个 token）
TEXT_IDS = [104, 20, 2869, 209, 32, 26, 8]  # 占位，实际由 tokens.tsv 编码


def encode(text, tokens_path):
    """极简 spm 编码，够用于 profile。"""
    id2tok = {}
    tok2id = {}
    with open(tokens_path, encoding="utf-8") as f:
        for idx, line in enumerate(f):
            line = line.rstrip("\n")
            if not line:
                continue
            tok2id[line] = idx
    unk = tok2id.get("<unk>", 2)
    ids = []
    for word in text.lower().split():
        key = "\u2581" + word
        if key in tok2id:
            ids.append(tok2id[key])
        else:
            # 退化成逐字符
            for ch in word:
                ids.append(tok2id.get(ch, unk))
    return ids, tok2id.get("</s>", 0)


def agg_trace(path, last_run_only=False):
    """解析 chrome trace，返回 (按算子聚合, 按节点聚合)。

    last_run_only=True 时只取时间轴上**最后一次**顶层 If 的窗口内事件
    （decoder 每次 run 只有一个顶层 If，可借此切出单步）。
    end_profiling() 会把 session 创建以来的所有 run 都落盘，含预热，
    不切窗口的话除以步数会算错。
    """
    with open(path, encoding="utf-8") as f:
        data = json.load(f)

    # ORT 1.3x 直接输出顶层数组，老版本是 {"traceEvents": [...]}
    if isinstance(data, dict):
        data = data.get("traceEvents", [])

    raw = []
    for e in data:
        if e.get("cat") != "Node" or "dur" not in e:
            continue
        raw.append(e)

    if last_run_only:
        tops = [e for e in raw if e.get("args", {}).get("op_name") == "If"]
        if tops:
            t0 = max(tops, key=lambda e: e.get("ts", 0))
            lo = t0["ts"]
            hi = t0["ts"] + t0["dur"]
            raw = [e for e in raw if lo <= e.get("ts", -1) <= hi]

    events = []
    for e in raw:
        args = e.get("args", {})
        events.append(
            {
                "op": args.get("op_name", "?"),
                "name": e.get("name", "?"),
                "dur": float(e["dur"]),  # 微秒
                "thread": e.get("tid", 0),
            }
        )

    by_op = collections.defaultdict(float)
    by_op_cnt = collections.Counter()
    by_node = collections.defaultdict(float)
    by_node_op = {}
    for e in events:
        by_op[e["op"]] += e["dur"]
        by_op_cnt[e["op"]] += 1
        key = e["name"]
        by_node[key] += e["dur"]
        by_node_op[key] = e["op"]

    return by_op, by_op_cnt, by_node, by_node_op, events


def report(title, path, top_ops=12, top_nodes=12, last_run_only=False):
    by_op, cnt, by_node, node_op, events = agg_trace(path, last_run_only)
    total = sum(by_op.values())
    n_events = len(events)
    if total == 0:
        print(f"  [{title}] 没有采到 Node 事件")
        return

    print(f"\n{'=' * 78}")
    print(f"  {title}   节点调用 {n_events} 次，累计 {total / 1000:.0f} ms")
    print(f"{'=' * 78}")

    print(f"\n  -- 按算子类型（Top {top_ops}）--")
    print(f"  {'算子':<34}{'调用':>7}{'累计ms':>11}{'占比':>8}{'单次us':>10}")
    for op, dur in sorted(by_op.items(), key=lambda kv: -kv[1])[:top_ops]:
        # If 是父节点，dur 含整个子图，不能和叶子算子并列比较
        mark = " (含子图)" if op == "If" else ""
        print(
            f"  {op:<34}{cnt[op]:>7}{dur / 1000:>11.1f}"
            f"{dur / total * 100:>7.1f}%{dur / cnt[op]:>10.0f}{mark}"
        )
    rest = total - sum(d for _, d in sorted(by_op.items(), key=lambda kv: -kv[1])[:top_ops])
    if rest > 0:
        print(f"  {'(其余 %d 种)' % (len(by_op) - top_ops):<34}{'':>7}{rest / 1000:>11.1f}{rest / total * 100:>7.1f}%")

    print(f"\n  -- 按单节点（Top {top_nodes}）--")
    print(f"  {'节点名':<46}{'算子':<20}{'累计ms':>9}")
    for name, dur in sorted(by_node.items(), key=lambda kv: -kv[1])[:top_nodes]:
        short = name if len(name) <= 44 else name[:41] + "..."
        print(f"  {short:<46}{node_op[name]:<20}{dur / 1000:>9.1f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("encoder", nargs="?", default=None)
    ap.add_argument("decoder", nargs="?", default=None)
    ap.add_argument("--steps", type=int, default=8)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--text", default="Save changes before closing?")
    args = ap.parse_args()

    home = os.path.expanduser("~")
    default_dir = os.path.join(home, "AppData", "Roaming", "ShotGlance", "models", "opus-mt-en-zh")
    enc_path = args.encoder or os.path.join(default_dir, "encoder.onnx")
    dec_path = args.decoder or os.path.join(default_dir, "decoder.onnx")
    tokens_path = os.path.join(os.path.dirname(enc_path), "tokens.tsv")
    if not os.path.exists(tokens_path):
        tokens_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
            "src-tauri", "assets", "mt", "tokens.tsv",
        )

    for p in (enc_path, dec_path, tokens_path):
        if not os.path.exists(p):
            sys.exit(f"文件不存在：{p}")

    so = ort.SessionOptions()
    so.intra_op_num_threads = args.threads
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    so.enable_profiling = True
    so.profile_file_prefix = os.path.join(tempfile.gettempdir(), "sgprof")

    print(f"encoder : {enc_path}")
    print(f"decoder : {dec_path}")
    print(f"threads : {args.threads}    steps: {args.steps}")

    ids, eos = encode(args.text, tokens_path)
    src = ids[:510] + [eos]
    L = len(src)
    print(f"源序列 : {L} token  {args.text!r}")

    # ---------- encoder ----------
    enc = ort.InferenceSession(enc_path, so, providers=["CPUExecutionProvider"])
    feeds = {
        "input_ids": np.array([src], np.int64),
        "attention_mask": np.ones((1, L), np.int64),
    }
    enc.run(None, feeds)  # 预热，不进 profile
    enc.run(None, feeds)
    hidden = enc.run(None, feeds)[0]
    enc_trace = enc.end_profiling()
    report("ENCODER  1 次前向（预热后）", enc_trace)

    # ---------- decoder ----------
    dec = ort.InferenceSession(dec_path, so, providers=["CPUExecutionProvider"])
    names = [o.name for o in dec.get_outputs()]
    past_src = [i for i in [x.name for x in dec.get_inputs()] if i.startswith("past_key_values.")]
    mask = np.ones((1, L), np.int64)
    hidden = np.ascontiguousarray(hidden, dtype=np.float32)

    def empty_past():
        return {
            f"past_key_values.{l}.{k}.{v}": np.zeros((1, DEC_HEADS, 0, DEC_HEAD_DIM), np.float32)
            for l in range(LAYERS)
            for k in ("decoder", "encoder")
            for v in ("key", "value")
        }

    past = empty_past()
    tok = PAD
    # 预热 3 步（不 profile）
    for step in range(3):
        f = dict(past)
        f["encoder_attention_mask"] = mask
        f["input_ids"] = np.array([[tok]], np.int64)
        f["encoder_hidden_states"] = hidden
        f["use_cache_branch"] = np.array([step > 0])
        outs = dict(zip(names, dec.run(None, f)))
        for k, v in outs.items():
            if k.startswith("present.") and not (step > 0 and ".encoder." in k):
                past[k.replace("present.", "past_key_values.")] = v
        tok = int(outs["logits"][0, -1].argmax())

    # 起 profile，跑 N 步
    for step in range(args.steps):
        f = dict(past)
        f["encoder_attention_mask"] = mask
        f["input_ids"] = np.array([[tok]], np.int64)
        f["encoder_hidden_states"] = hidden
        f["use_cache_branch"] = np.array([True])
        outs = dict(zip(names, dec.run(None, f)))
        for k, v in outs.items():
            if k.startswith("present.") and ".decoder." in k:
                past[k.replace("present.", "past_key_values.")] = v
        tok = int(outs["logits"][0, -1].argmax())
    dec_trace = dec.end_profiling()
    report(
        "DECODER  单步（cache 分支，已预热，按 If 窗口切出）",
        dec_trace,
        top_ops=14,
        top_nodes=14,
        last_run_only=True,
    )

    print(f"\n原始 trace 保留在：\n  {enc_trace}\n  {dec_trace}")


if __name__ == "__main__":
    main()
