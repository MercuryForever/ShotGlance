"""离线翻译的参考实现（构建期一次性验证，不随应用分发）。

目的：在把逻辑搬进 Rust 之前，先确认三件事
  1. Xenova 导出的 int8 ONNX 与 source.spm + vocab.json 是配套的
  2. encoder-decoder 的输入名、输出结构、特殊 token 约定
  3. 贪婪解码能不能出通顺中文（vs 官方 beam=4 的差距有多大）

关键点：spm 词表只有 32000 个 piece，但模型 embedding 是 65001 行。
两者的桥是 vocab.json —— 用 piece 字符串查表做重映射，不是简单的 id 偏移。
"""

import sys
import json
import time
import numpy as np
import onnxruntime as ort
import sentencepiece as spm

D = "./"
PAD = 65000
EOS = 0
UNK = 1
MAX_NEW = 96


def load():
    vocab = json.load(open(D + "vocab.json", encoding="utf-8"))
    id2tok = {v: k for k, v in vocab.items()}

    src = spm.SentencePieceProcessor()
    src.Load(D + "source.spm")

    enc = ort.InferenceSession(D + "encoder_model_quantized.onnx",
                               providers=["CPUExecutionProvider"])
    dec = ort.InferenceSession(D + "decoder_model_quantized.onnx",
                               providers=["CPUExecutionProvider"])
    print("encoder inputs :", [(i.name, i.shape) for i in enc.get_inputs()])
    print("encoder outputs:", [o.name for o in enc.get_outputs()])
    print("decoder inputs :", [(i.name, i.shape) for i in dec.get_inputs()])
    print("decoder outputs:", [(o.name, o.shape) for o in dec.get_outputs()][:3], "...")
    return vocab, id2tok, src, enc, dec


def encode(src, vocab, text):
    pieces = [src.id_to_piece(i) for i in src.encode(text, out_type=int)]
    return [vocab.get(p, UNK) for p in pieces]


def decode(id2tok, ids):
    s = "".join(id2tok.get(i, "") for i in ids)
    return s.replace("\u2581", " ").strip()


def translate(ctx, text, max_new=MAX_NEW):
    vocab, id2tok, src, enc, dec = ctx

    # Marian 约定：源串末尾补 </s>(0)。漏掉仍能翻，但长句断句会变差。
    ids = encode(src, vocab, text) + [EOS]
    input_ids = np.array([ids], dtype=np.int64)
    mask = np.ones_like(input_ids)

    t0 = time.perf_counter()
    hidden = enc.run(None, {
        "input_ids": input_ids,
        "attention_mask": mask,
    })[0]
    t_enc = (time.perf_counter() - t0) * 1000

    cur = [PAD]
    out = []
    t0 = time.perf_counter()
    for _ in range(max_new):
        logits = dec.run(None, {
            "input_ids": np.array([cur], dtype=np.int64),
            "encoder_attention_mask": mask,
            "encoder_hidden_states": hidden,
        })[0]
        row = logits[0, -1].copy()
        row[PAD] = -1e9          # bad_words_ids = [[65000]]
        nxt = int(np.argmax(row))
        if nxt == EOS:
            break
        cur.append(nxt)
        out.append(nxt)
    t_dec = (time.perf_counter() - t0) * 1000

    return decode(id2tok, out), t_enc, t_dec, len(out)


if __name__ == "__main__":
    ctx = load()
    cases = [
        "Hello, world.",
        "Press any key to continue.",
        "The quick brown fox jumps over the lazy dog.",
        "Failed to connect to the server. Please check your network settings.",
        "Save changes before closing?",
    ]
    if len(sys.argv) > 1:
        cases = sys.argv[1:]

    total_enc = total_dec = 0
    for c in cases:
        txt, te, td, n = translate(ctx, c)
        total_enc += te
        total_dec += td
        print(f"[{n:2d} tok | enc {te:6.1f} | dec {td:6.1f} ms] {c}")
        print(f"   -> {txt}")
    print(f"\nenc {total_enc:.0f} ms | dec {total_dec:.0f} ms")
