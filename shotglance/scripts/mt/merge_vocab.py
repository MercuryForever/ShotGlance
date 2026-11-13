"""把 vocab.json + source.spm 合成一份 Rust 侧可直接吃的词表。

为什么要合并：编码需要 (piece -> 模型id, score)，解码需要 (模型id -> piece)，
但 score 只在 spm 里、模型 id 只在 vocab.json 里，分两个文件读两遍很啰嗦。

输出 tokens.tsv：`id<TAB>score<TAB>piece`
  - 覆盖 vocab.json 全部 65001 项（解码要完整，含 <pad>）
  - score 取自 source.spm；spm 里没有的 piece 记为 -100（不可被编码命中）
  - <unk>/<s>/</s>/<pad> 的 score 强制 0，避免影响 Viterbi 选择
"""

import json
import sys
import sentencepiece as spm

D = sys.argv[1] if len(sys.argv) > 1 else "./"

vocab = json.load(open(D + "vocab.json", encoding="utf-8"))
sp = spm.SentencePieceProcessor()
sp.Load(D + "source.spm")

n = sp.get_piece_size()
score = {}
for i in range(n):
    score[sp.id_to_piece(i)] = sp.get_score(i)

# unk 的分数取 spm 里的实际值；其余特殊 token 置 0
rows = []
for piece, mid in vocab.items():
    s = score.get(piece, -100.0)
    if piece in ("<unk>", "<s>", "</s>", "<pad>"):
        s = 0.0
    if "\t" in piece or "\n" in piece:
        piece = piece.replace("\t", " ").replace("\n", " ")
    rows.append((mid, s, piece))

rows.sort(key=lambda r: r[0])
with open(D + "tokens.tsv", "w", encoding="utf-8", newline="\n") as f:
    for mid, s, piece in rows:
        f.write(f"{mid}\t{s!r}\t{piece}\n")

missing = sum(1 for _, _, p in rows if p not in score and p not in ("<pad>",))
print(f"tokens.tsv: {len(rows)} 行，其中 {len(score)} 个 piece 带 spm 分数，"
      f"{missing} 个只在 vocab.json 里")
print("unk score =", score.get("<unk>"))
