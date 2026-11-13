"""对比贪婪 vs beam=4，并测长文本的真实表现（构建期一次性）。

结论会决定 Rust 侧实现哪种解码：
  - 如果 beam 提升明显，就实现 batch 化的 beam（4 条候选并成一次前向）
  - 如果提升有限，就保持贪婪，把复杂度省下来
"""

import json
import time
import numpy as np
import onnxruntime as ort
import sentencepiece as spm

D = "./"
PAD, EOS, UNK = 65000, 0, 1

vocab = json.load(open(D + "vocab.json", encoding="utf-8"))
id2tok = {v: k for k, v in vocab.items()}
src = spm.SentencePieceProcessor()
src.Load(D + "source.spm")
enc = ort.InferenceSession(D + "encoder_model_quantized.onnx", providers=["CPUExecutionProvider"])
dec = ort.InferenceSession(D + "decoder_model_quantized.onnx", providers=["CPUExecutionProvider"])


def encode(text):
    return [vocab.get(src.id_to_piece(i), UNK) for i in src.encode(text, out_type=int)] + [EOS]


def decode(ids):
    return "".join(id2tok.get(i, "") for i in ids).replace("\u2581", " ").strip()


def prep(text):
    ids = encode(text)
    input_ids = np.array([ids], dtype=np.int64)
    mask = np.ones_like(input_ids)
    hidden = enc.run(None, {"input_ids": input_ids, "attention_mask": mask})[0]
    return mask, hidden


def greedy(mask, hidden, max_new=96):
    cur = [PAD]
    out = []
    for _ in range(max_new):
        logits = dec.run(None, {
            "input_ids": np.array([cur], dtype=np.int64),
            "encoder_attention_mask": mask,
            "encoder_hidden_states": hidden,
        })[0][0, -1].copy()
        logits[PAD] = -1e9
        nxt = int(np.argmax(logits))
        if nxt == EOS:
            break
        cur.append(nxt)
        out.append(nxt)
    return out


def beam(mask, hidden, width=4, max_new=96, len_pen=1.0):
    """标准 beam search。每步把全部候选并成一个 batch 前向，
    这样一步只付一次 ONNX 调用开销，而不是 width 次。"""
    seqs = [[PAD]]
    scores = [0.0]
    done = []
    for step in range(max_new):
        batch = np.array(seqs, dtype=np.int64)
        reps = np.repeat(hidden, len(seqs), axis=0)
        m = np.repeat(mask, len(seqs), axis=0)
        logits = dec.run(None, {
            "input_ids": batch,
            "encoder_attention_mask": m,
            "encoder_hidden_states": reps,
        })[0][:, -1, :].copy()
        logits[:, PAD] = -1e9
        logp = logits - np.log(np.exp(logits - logits.max(1, keepdims=True)).sum(1, keepdims=True)) - logits.max(1, keepdims=True)

        cand = []
        for i, s in enumerate(seqs):
            top = np.argpartition(-logp[i], width)[:width]
            for t in top:
                cand.append((scores[i] + float(logp[i, t]), s + [int(t)]))
        cand.sort(key=lambda x: x[0] / (len(x[1]) ** len_pen), reverse=True)

        seqs, scores = [], []
        for sc, s in cand[:width]:
            if s[-1] == EOS:
                done.append((sc / (len(s) ** len_pen), s[:-1]))
            else:
                seqs.append(s)
                scores.append(sc)
        if not seqs:
            break
    done.extend((scores[i] / (len(seqs[i]) ** len_pen), seqs[i][1:]) for i in range(len(seqs)))
    done.sort(key=lambda x: x[0], reverse=True)
    return done[0][1]


CASES = [
    "Hello, world.",
    "Press any key to continue.",
    "Save changes before closing?",
    "Failed to connect to the server. Please check your network settings.",
    """To install the package, run the following command in your terminal.
After installation, restart the application so the changes take effect.
Note that administrator privileges are required on Windows.""",
]

for c in CASES:
    mask, hidden = prep(c)
    t0 = time.perf_counter()
    g = greedy(mask, hidden)
    tg = (time.perf_counter() - t0) * 1000
    t0 = time.perf_counter()
    b = beam(mask, hidden)
    tb = (time.perf_counter() - t0) * 1000
    print(f"\n=== {c[:58]}{'...' if len(c) > 58 else ''}")
    print(f"  greedy({len(g):2d}tok {tg:5.0f}ms): {decode(g)}")
    print(f"  beam4 ({len(b):2d}tok {tb:5.0f}ms): {decode(b)}")
