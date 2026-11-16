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
# decoder 必须用 **merged** 版：它带 24 个 past_key_values 输入，
# 才能做 KV cache 增量解码。普通 decoder_model_quantized.onnx 只能全序列重算。
curl -L "$BASE/Xenova/opus-mt-en-zh/resolve/main/onnx/decoder_model_merged_quantized.onnx" -o decoder.onnx
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

# 5. （可选，强烈建议）生成词表投影加速版
#    decoder.int8.onnx 存在时运行时优先用它，单步 14.8ms -> 2.4ms
python transform_lmhead.py --src "$APPDATA/ShotGlance/models/opus-mt-en-zh/decoder.onnx" \
                           --out "$APPDATA/ShotGlance/models/opus-mt-en-zh/decoder.int8.onnx"
python verify_lmhead.py "$APPDATA/ShotGlance/models/opus-mt-en-zh/decoder.onnx" \
                        "$APPDATA/ShotGlance/models/opus-mt-en-zh/decoder.int8.onnx"
```

## 各脚本职责

| 文件 | 作用 |
|---|---|
| `export_vocab.py` | 把 spm 的 `id/score/piece` 导成文本（排查用，最终产物是 merge 之后的） |
| `merge_vocab.py` | **关键**：把 `vocab.json` 的模型 id 和 `source.spm` 的分数合成 `tokens.tsv` |
| `ref.py` | ONNX 参考推理。跑通它才说明模型可用，也是 Rust 实现的对照基线 |
| `cmp.py` | 贪婪 vs beam=4 的质量/耗时对比（结论：贪婪胜出） |
| `bench_decode.py` | 全序列重算 vs merged+KV cache 的耗时曲线（2026-11-16 提速的依据） |
| `profile_ops.py` | 算子级剖面，按 ONNX 节点聚合耗时。找瓶颈第一步 |
| `probe_lmhead.py` | 打印词表投影附近的图结构（谁在每步转置/反量化 33MB 权重） |
| `probe_pattern.py` | 打印 FC/attention 的正确 int8 写法，作为改造模板 |
| `transform_lmhead.py` | **加速**：把 lm_head 改成 int8 直算，产出 `decoder.int8.onnx` |
| `verify_lmhead.py` | 校验改造前后 logits/argmax 一致，并对比单步耗时 |

## 词表投影：为什么值得单独改一次

merged decoder 的 `If` 子图里，词表投影是这么写的：

```
shared.weight (uint8 [65001,512], 33MB 常量)
  -> Transpose -> DequantizeLinear -> MatMul
```

权重要么是常量，要么不是，而 ORT 的常量折叠不跨 `If` 子图——于是**每生成一个
token 都重新转置 33 MB、再展开成 133 MB fp32**。实测单步 14.8 ms，其中约 12 ms
耗在这两步上，跟模型主干完全无关。

`transform_lmhead.py` 把同一份权重换成预转置的 int8 副本，投影走
`DynamicQuantizeLinear + MatMulInteger`，只反量化 1×65001 的输出：

```
单步 14.84 ms -> 2.37 ms     长序列（60 token）同样 5.9x
首步 logits 最大误差 0.234（值域 ±9.8），argmax 序列 20/20 一致
```

代价是多 32 MB 的 int8 转置权重（57MB -> 89MB）。运行时反而省内存：不再每步
分配 133 MB 的 fp32 权重临时张量。

改造后 profile 里最大的单项变成 lm_head 的 int8 GEMM（约 1.1 ms，占单步 8.6%），
其次是 KV cache 的 `Concat`(109 次) 和 `Reshape`(98 次) 这类布局算子。

## 最容易踩的坑

`source.spm` 只有 **32000** 个 piece，模型 embedding 却是 **65001** 行。两者的桥是
`vocab.json`——用 piece **字符串**查表做重映射，不是简单的 id 偏移。直接把 spm 的 id
喂给模型，输出会是完全无关的乱码，而且不报错。

`tokens.tsv` 的格式是 `id<TAB>score<TAB>piece`，65001 行全都要（解码需要完整的
id → piece 反查，包括 `<pad>`）。
