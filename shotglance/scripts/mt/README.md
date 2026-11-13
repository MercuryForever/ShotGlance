# 离线翻译模型的构建期脚本

这些脚本**不参与构建，也不进安装包**。它们只做一件事：把 HuggingFace 上的
Opus-MT 模型转成 Rust 侧能直接吃的形态。换语言对、换模型版本、或者想换量化档位时
才需要重跑。

运行时依赖：`sentencepiece`、`onnxruntime`、`numpy`（Python 3.13 验证过）。

## 流程

```bash
# 1. 拉模型文件（HuggingFace 直连不通时用镜像）
BASE=https://hf-mirror.com
curl -L "$BASE/Xenova/opus-mt-en-zh/resolve/main/config.json"            -o config.json
curl -L "$BASE/Xenova/opus-mt-en-zh/resolve/main/source.spm"             -o source.spm
curl -L "$BASE/Xenova/opus-mt-en-zh/resolve/main/target.spm"             -o target.spm
curl -L "$BASE/Xenova/opus-mt-en-zh/resolve/main/onnx/encoder_model_quantized.onnx" -o encoder.onnx
curl -L "$BASE/Xenova/opus-mt-en-zh/resolve/main/onnx/decoder_model_quantized.onnx" -o decoder.onnx
# vocab.json 在原始仓库，Xenova 的镜像里没有——缺了它 id 对不上，译文全是乱码
curl -L "$BASE/Helsinki-NLP/opus-mt-en-zh/resolve/main/vocab.json"       -o vocab.json

# 2. 跑参考实现，确认模型和词表是配套的
python ref.py
python cmp.py          # 对比贪婪 vs beam，决定解码策略

# 3. 合成 Rust 侧词表
python merge_vocab.py ./
cp tokens.tsv ../../src-tauri/assets/mt/tokens.tsv

# 4. 装进本地模型目录
mkdir -p "$APPDATA/ShotGlance/models/opus-mt-en-zh"
cp encoder.onnx decoder.onnx tokens.tsv "$APPDATA/ShotGlance/models/opus-mt-en-zh/"
```

## 各脚本职责

| 文件 | 作用 |
|---|---|
| `export_vocab.py` | 把 spm 的 `id/score/piece` 导成文本（排查用，最终产物是 merge 之后的） |
| `merge_vocab.py` | **关键**：把 `vocab.json` 的模型 id 和 `source.spm` 的分数合成 `tokens.tsv` |
| `ref.py` | ONNX 参考推理。跑通它才说明模型可用，也是 Rust 实现的对照基线 |
| `cmp.py` | 贪婪 vs beam=4 的质量/耗时对比（结论：贪婪胜出） |

## 最容易踩的坑

`source.spm` 只有 **32000** 个 piece，模型 embedding 却是 **65001** 行。两者的桥是
`vocab.json`——用 piece **字符串**查表做重映射，不是简单的 id 偏移。直接把 spm 的 id
喂给模型，输出会是完全无关的乱码，而且不报错。

`tokens.tsv` 的格式是 `id<TAB>score<TAB>piece`，65001 行全都要（解码需要完整的
id → piece 反查，包括 `<pad>`）。
