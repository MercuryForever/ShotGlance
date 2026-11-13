"""导出 SentencePiece 词表给 Rust 侧用（构建期一次性）。

Rust 不引 sentencepiece 库（那个 crate 要 cmake + protobuf 编译 C++，
给一个 20 MB 的安装包加这种构建依赖不划算）。改为把 spm 的
"id / score / piece" 三元组导出成纯文本，Rust 端自己跑 Viterbi。

格式：每行 `id<TAB>score<TAB>piece`，piece 里可能含任意字符但不含 TAB/换行
（spm 的 piece 不会含这两个）。

注意：source 与 target 的 spm 必须分开导出。Marian 虽然是共享词表，
但两个文件的 id 顺序并不保证一致，混用会让译文变成乱码。
"""

import sys
import sentencepiece as spm


def dump(path, out):
    sp = spm.SentencePieceProcessor()
    sp.Load(path)
    n = sp.get_piece_size()
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        for i in range(n):
            piece = sp.id_to_piece(i)
            # piece 里出现制表符或换行会破坏行格式，spm 正常情况下不会
            piece = piece.replace("\t", " ").replace("\n", " ")
            f.write(f"{i}\t{sp.get_score(i)!r}\t{piece}\n")
    print(f"{out}: {n} pieces")
    return n


if __name__ == "__main__":
    d = sys.argv[1] if len(sys.argv) > 1 else "./"
    a = dump(d + "source.spm", d + "vocab.src.txt")
    b = dump(d + "target.spm", d + "vocab.tgt.txt")
    if a != b:
        print(f"警告：source/target 词表大小不一致（{a} vs {b}）", file=sys.stderr)
