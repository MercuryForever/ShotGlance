# HANDOFF — ShotGlance 工作区状态与迁移说明

> 目的：换一台机器（或隔几周回来）时，只看这一份文档就能恢复开发环境并知道下一步做什么。
> 最后更新：2026-11-12 11:36，对应工作区根目录下**最新的** `shotglance-snapshot-*.zip`（约 46 MB / 59 文件）
> 配套文档：`ENV-MIGRATION.md`（**迁移优先看这份**，含 env-bundle 导出/恢复）、上级目录 `README.md`（工作区索引）、`README.md`（技术决策与实测细节）、`.workbuddy/memory/`（踩坑流水账）

---

## 0. 三十秒版

```powershell
# 1) 解压快照到任意目录，进入 shotglance\
npm ci
# 2) 确认 onnxruntime.dll、models\、assets\mt\tokens.tsv 都在 src-tauri\ 下
npm run tauri:build          # 需要 PATH 里有 cargo
```

产物 `src-tauri\target\release\bundle\nsis\ShotGlance_0.1.0_x64-setup.exe`（22 MB）。
不想装工具链就直接跑快照里的 `release\ShotGlance_0.1.0_x64-setup.exe`。

---

## 1. 项目现状

**ShotGlance**：截图 → OCR → 翻译 → 悬浮回显的轻量桌面工具。Rust + Tauri 2，托盘常驻，`Ctrl+Shift+S` 唤起。

硬指标与实际达成：

| 指标 | 目标 | 实测 | 状态 |
|---|---|---|---|
| 安装包 | < 150 MB | 21.91 MiB | 达标 |
| 空闲内存 | < 80 MB | 托盘 11.5 MB / 面板回收后 32.9 MB | 达标 |
| OCR 峰值 | < 300 MB | 152.7 MB（引擎加载后） | 达标 |
| 冷启动到首窗 | < 2 s | 87–112 ms | 达标 |
| 截图耗时 | < 300 ms | 稳态 130 ms（冷 351 ms） | 达标 |
| 离线翻译（≤30 输出 token） | < 500 ms | 25–80 ms（§1.5 之后；原 133–404 ms） | 达标 |
| 单文件免安装 | 要求 | NSIS currentUser，WebView2 embedBootstrapper | 达标（Windows） |

已完成 / 未完成：

- [x] 项目骨架、纯托盘启动、按需建窗、空闲 watchdog 回收（destroy 而非 hide）
- [x] 全局热键 `Ctrl+Shift+S`，注册失败降级为托盘触发
- [x] 截屏采集（xcap + WGC，多屏枚举，JPEG q85 预览）
- [x] PP-OCRv4 检测 + 识别（ort `load-dynamic` + 自写前后处理）
- [x] OCR 引擎按需加载，面板销毁即 `ocr::unload()` 回到 32.9 MB
- [x] NSIS 离线打包链路（NSIS 缓存可从 `scripts/vendor/` 恢复）
- [x] **区域框选**（`region_begin` / `region_finish` / `region_cancel`，见 1.1）
- [x] **翻译层**（5 个引擎可切换 + auto 对冲竞速 + 离线 Opus-MT，见 1.2）
- [x] **译文悬浮层**（贴原文框显示，不遮挡上下文，见 1.3）
- [x] **框选后自动翻译**（默认开，设置里可关）

### 1.1 区域框选（2026-11-11 实现）

交互：面板点「框选」→ 面板窗口当场摘掉标题栏、铺满目标显示器、置顶，底下压着"按下那一刻"的冻结帧 → 拖出矩形，松手立即裁剪 + OCR → Esc / 右键取消。

三个关键设计：

| 决策 | 理由 |
|---|---|
| **复用面板窗口**拉成全屏，而不是新开一个 overlay 窗口 | 多一个 WebView2 窗口就多 5–6 个子进程（约 290 MB），选区是瞬时交互，不值得 |
| **冻结一帧并暂存**（`PENDING`），裁剪和识别都吃这一帧 | 松手后重新抓屏的话，弹窗/滚动/悬停高亮会让内容和眼睛看到的错位 |
| **坐标换算交给渲染矩形反算**（`frameW / rect.width`） | 不写死 DPI 缩放，多屏不同缩放比、窗口过渡态都自动成立 |

命令与参数：

| 命令 | 作用 |
|---|---|
| `region_begin{monitor}` | 抓帧 → 暂存 → 窗口进入选区形态；返回整屏 JPEG 与帧尺寸 |
| `region_finish{x,y,w,h,runOcr}` | 还原窗口 → 裁剪 → 可选 OCR → 返回裁剪图 + 结果（坐标是帧像素，可为小数） |
| `region_cancel` | 还原窗口 + 丢弃暂存帧 |

已知取舍：裁剪后识别只对选区内的行做前向，所以时延从整屏的 811 ms（83 行）降到"行数 × 8.4 ms"级别；`ocr::recognize` 不受影响，仍是同一套 det/rec。

**验证状态（2026-11-11，VMware 虚拟机 / 2560×1440 / DPI 100%）**

| 项目 | 结果 |
|---|---|
| `cargo check` / release 打包 | 通过（21.22 MiB 安装包） |
| 裁剪取整与越界夹取 | 通过（`SHOTGLANCE_CROP` 自检：`-50,500,2000,900` → `at 0,500 / 1000×60`） |
| 端到端（选区 → 裁剪 → 识别） | 通过：`req=(300,300,600,260)`，识别 16 行 / 180 ms，面板回显裁剪图 |
| 坐标映射 1:1 | 通过：窗口客户区 2560×1440，`rect=0,0 2560×1440`，`dpr=1` |
| **选区遮罩的视觉呈现** | **未能确认**：窗口 resize 到 2560×1440 后，WebView2 只重绘了新出现的文字，遮罩底色与底图没有重新绘制（屏幕上残留旧尺寸的一帧）。同尺寸的普通窗口与该 VM 的 GPU 合成有关，需在真机复看 |

如果真机上也复现"遮罩不铺满"，替代方案是给选区单独建一个**创建时就按全屏尺寸**的窗口（避免创建后再 resize），代价是多一份 WebView2 子进程（约 290 MB，仅选区那几秒）。


多屏注意：选区面用的是 Tauri 的显示器几何，帧来自 xcap 的同序号显示器。两者枚举顺序在单屏下必然一致；多屏若顺序不同，会出现"底图是 A 屏、窗口在 B 屏"。这一条尚未在多屏机器上实测。


实测数字（同一台机器，2560×1440 单屏）：

| 场景 | 检测 | 识别 | 合计 | 结果 |
|---|---|---|---|---|
| 1000×560 基准图（8 行） | 80 ms | 161 ms | 242 ms | 8/8 行，置信 0.97–1.00 |
| 2560×1440 全屏密集桌面 | 112 ms | 699 ms | 811 ms | 83 行 |

已知错字：`阈→阔`、`」→]`；中英混排时词间空格偶发丢失（模型对 space 类判别弱，PaddleOCR 同源同病）。
识别是瓶颈，约 **8.4 ms/行**；全屏 80+ 行才成为主项，做区域框选后基本可忽略。

---

### 1.2 翻译层（2026-11-11 实现）

五个引擎并存，用户在下拉里切换；`auto` 是一条策略而不是引擎。

| id | 引擎 | 需要 | 说明 |
|---|---|---|---|
| `deepl` | DeepL | API Key | 质量最好。free 档 `api-free.deepl.com`，pro 档 `api.deepl.com`，用错域名直接 403，所以做成开关 |
| `openai` | OpenAI 兼容 | 端点 + Key + 模型 | 任何兼容服务：OpenAI、DeepSeek、通义、one-api、本地 Ollama |
| `google` | Google 翻译 | — | 网页版 gtx 端点，免密钥，质量好但为非公开接口 |
| `mymemory` | MyMemory | — | 免密钥，匿名 5000 字符/天，填 email 提到 50000 |
| `offline` | 本地 Opus-MT | 113 MB 模型（可选跑脚本 +32 MB 换 6 倍速） | 完全离线，装一次永久可用 |

**`auto` 的行为**：在线引擎**对冲竞速**（第一个立刻发，之后每 250 ms 补一个，谁先给出有效译文用谁），全部失败再串行落到离线模型。

- 为什么不是"全部同时发"：MyMemory 免费额度按字符计，无脑并发会把额度白白烧光。对冲下正常网络只有一个引擎真正发出请求。
- 为什么需要竞速：本机 googleapis 直接连不上，串行降级每次翻译都要先卡满 5 秒连接超时。竞速后实测 **1243 ms → 613 ms**（第二轮因熔断跳过 google）。
- **失败熔断**：某在线引擎失败后 90 秒内在 auto 链里被跳过，避免反复付超时成本。用户显式指定该引擎时不跳（那多半是刚改完配置在验证）。

**离线引擎的实现要点**

1. **模型外置**。quantized encoder 52.9 MB + decoder 60.2 MB = 113 MB。打进安装包会让 22 MB 变成 135 MB，直接顶到 150 MB 红线，而它只对选了离线的用户有价值。放 `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`，应用内一键下载，也可手动放文件。
2. **词表随包**（`assets/mt/tokens.tsv`，1.8 MB）。它和模型版本绑死，没必要每次下载。
3. **spm 与 vocab 是两套 id，必须过 `vocab.json` 重映射**。`source.spm` 只有 32000 个 piece，模型 embedding 却是 65001 行；`vocab.json` 给出 piece → 模型 id 的映射。直接拿 spm 的 id 喂模型只会得到乱码——这是接 Marian 最容易踩的坑。`tokens.tsv` 就是把两者合成一份 `id / score / piece`。
4. **自己实现 SentencePiece 编码**（`Vocab::encode`）：空白折叠成 `▁` + 行首补 `▁` + Unigram Viterbi。不引 `sentencepiece` crate（要 cmake 编 C++）也不引 `tokenizers`（需要 tokenizer.json，Xenova 仓库只有 .spm）。`unk` 的分数取 `min_score - 10`，压不到最低的话 Viterbi 会认为"整句都是 unk"更优。
5. **贪婪解码，不用 beam=4**。实测 beam 慢 4 倍，长文本质量差别很小；短句的差异主要来自模型本身而非搜索策略。
6. **中文排版后处理**（`polish`）：spm 解码后词间是空格，中文里全部要去掉；模型输出的半角 `,` `?` 在中文语境转全角。半角标点转全角时**空格要提前删**，否则会留下"吗 ？"这种两侧带空的全角标点（2026-11-16 修，3 个单测护住）。
7. **KV cache 增量解码**（2026-11-16，见 §1.4）。
8. **词表投影 int8 化**（2026-11-16，见 §1.5）：`decoder.int8.onnx` 存在时优先加载，单步 14.84 → 2.37 ms。

### 1.4 离线解码提速：merged decoder + KV cache（2026-11-16 实现）

原实现每步都把整段前缀重新喂给 decoder（`[1, seq]` → 全量重算），seq 越长越慢。改成 merged decoder 后每步只算 1 个新 token，历史留在 cache 里。

**换模型**：`decoder_model_quantized.onnx` → `decoder_model_merged_quantized.onnx`。两者体积几乎相同（60.2 MB），merged 版把"首步无 cache"和"后续步带 cache"两条路径合进一个图，多出 1 个 `use_cache_branch` 输入和 24 个 `past_key_values.N.{decoder,encoder}.{key,value}` 输入/输出。

**三个非踩不可的坑**（都记在 §5）：

1. **`use_cache_branch` 不能省**。首步必须置假（走"无 cache"分支，cross-attention 的 K/V 由 encoder 输出算出），之后置真。恒定置真会拿空 past 当 cross K/V，直接报 Reshape 错。
2. **past 不能用 0 长度占位**。虽然 `past_key_values.*` 声明为 `[-1, 8, -1, 64]`，看起来 seq=0 合法，但 ONNX Runtime 的 Reshape 不接受 0 长度维——它报的是 `The dimension with value zero exceeds the dimension size of the input tensor`，位置在 `If` 子图里，非常不好认。正确做法是首步就喂 `[1, 8, 0, 64]` 的空张量，让 ORT 自己走无 cache 分支；不能像我一开始那样自作聪明塞 `[1, 8, 1, 64]` 的零占位。
3. **cache 分支给出的 `present.*.encoder.*` 是坏的**（形状 `[0, 8, 1, 64]`，batch 维变成 0）。cross-attention 的 K/V 整段解码都不变，所以只收首步的 present、之后不再更新 encoder 侧，否则第 3 步必崩。

**收益：看长度，不看倍数**（同一台机器，decode 段计时，4 线程）

| 输出 token 数 | 全序列重算 | merged + KV | 倍数 |
|---|---|---|---|
| 8 | 153 ms | 129 ms | 1.18× |
| 13 | 252 ms | 207 ms | 1.22× |
| 26 | 543 ms | 432 ms | 1.26× |
| 50 | 1229 ms | 848 ms | 1.45× |
| 80 | 2217 ms | 1360 ms | 1.63× |

**为什么不是文档原先估的 3–5 倍**：这台机器上每步的固定成本约 15 ms，其中最大一块是 65001 路输出投影（512×65001 int8 MatMul，每 token 33M MAC）。它跟序列长度无关，KV cache 消不掉。KV cache 只消掉随长度平方增长的那部分，所以短句只有 1.2×，长句才逐步逼近 3×。顺带验证了线程数不是瓶颈：1/2/4/8/20 线程分别是 20.8/17.4/17.2/17.0/23.2 ms/步，4 线程已饱和（也符合内存红线的取舍）。

**应用内实测**（`SHOTGLANCE_MT_BENCH=1`，离线引擎，取 4 次最小值）

| 文本 | token | 改造前 | 改造后 | 译文 |
|---|---|---|---|---|
| `Save changes before closing?` | 8 | 160 ms | 133 ms | 关闭前保存更改吗？ |
| `Failed to connect…` | 13 | 257 ms | 209 ms | 连接到服务器失败。请检查您的网络设置。 |
| 长句（38 词） | 26 | 517 ms | 404 ms | 应用程序无法完成请求的操作，因为网络连接中断。请验证您的代理配置，稍后再试。 |

**排查工具**：`SHOTGLANCE_MT_BENCH=1` 跑离线基准并把 decoder 的完整输入输出签名写进 `%TEMP%\shotglance-mt-bench.txt`；`offline::dump_io()` 可单独打印签名。换模型版本时先看这份签名再改代码。

### 1.5 词表投影 int8 化：单步再快 6 倍（2026-11-16 实现）

§1.4 结尾那句"要继续压只能动量化/算子层"就是这个。做了一轮算子级剖面，结论比预期更明确：**瓶颈不是模型主干，是导出图里一处写坏的词表投影**。

**剖面怎么做的**：`scripts/mt/profile_ops.py`，用 ORT 自带的 `enable_profiling` 落 chrome trace，按 ONNX 节点聚合耗时。必须按最后一次 `If` 的时间窗口切出"单步"——`end_profiling()` 会把 session 创建以来的所有 run（含预热）都写进去，直接除以步数一定算错。

**原图长这样**（merged decoder 的 `If` 子图内）：

```
shared.weight  uint8 [65001, 512]  33 MB 常量
  -> Transpose            每步重新转置 33 MB
  -> DequantizeLinear     每步展开成 133 MB fp32
  -> MatMul               512 x 65001 fp32 GEMM
```

权重是常量，ORT 的常量折叠**不跨 `If` 子图**，于是每生成一个 token 都白白重做一次转置和反量化。剖面：单步 40 ms（profile 口径）里 `DequantizeLinear` 占 27.8%、`MatMul` 占 7.1%，六层 transformer 的 FFN/attention 加起来只有 4%。

**改法**（`scripts/mt/transform_lmhead.py`）：把权重预转置成 int8 副本，投影走模型里 FC 层用的同一套模式。

```
DynamicQuantizeLinear(A)                    1 x 512，激活量化
MatMulInteger(q, W_T_q, a_zp, w_zp)         int8 GEMM -> 1 x 65001 int32
Mul(Cast(mm), a_scale * w_scale)            只反量化 1 x 65001
Add(out, final_logits_bias)
```

**收益**（`verify_lmhead.py`，同一进程内 A/B，4 线程取最小值）

| 项 | 原 decoder | int8 版 |
|---|---|---|
| 单步 | 14.84 ms | **2.37 ms**（6.3×） |
| 60 token 长序列 | 14.84 ms/步 | 2.52 ms/步（5.9×） |
| 模型体积 | 57.4 MB | 89.2 MB |
| 首步 logits 最大误差 | — | 0.234（值域 ±9.8） |
| argmax 序列 | — | 20/20 完全一致 |

**应用内实测**（`SHOTGLANCE_MT_BENCH=1`，离线引擎，4 次取最小值）

| 文本 | token | §1.4 后 | 本节后 | 累计 vs 原始 | 译文 |
|---|---|---|---|---|---|
| `Save changes before closing?` | 8 | 133 ms | **25 ms** | 6.1× | 关闭前保存更改吗？ |
| `Failed to connect…` | 13 | 209 ms | **43 ms** | 5.9× | 连接到服务器失败。请检查您的网络设置。 |
| 长句（38 词） | 26 | 404 ms | **80 ms** | 6.8× | 应用程序无法完成请求的操作，因为网络连接中断。请验证您的代理配置，稍后再试。 |

三句译文与改造前**逐字一致**。

**落地方式**：`decoder.int8.onnx` 是可选文件，`offline::decoder_name()` 探测到就直接用，探测不到退回 `decoder.onnx`——下载流程一行没改，用户不跑脚本也能正常用。体积代价 32 MB 只发生在"多存一份 int8 转置权重"上；运行时反而更省：不再每步分配 133 MB 的 fp32 权重临时张量。

**改造后单步的构成**（profile 口径 31 ms）：lm_head 的 int8 GEMM 1.1 ms（8.6%）、KV cache 的 `Concat` 109 次 2.6 ms、`Reshape` 98 次 1.9 ms。剩下的是布局算子，再压要动图结构，收益边际。

**两个实现坑**：

1. **`If` 的 then/else 各有一套同名节点**，不能靠 `id()` 或 `is` 区分——protobuf 的 repeated 容器每次访问都可能返回新的包装对象，`n is mm` 恒为假，替换会静默失效。一律按节点名匹配。
2. **新节点必须插在 MatMul 原来的位置**，append 到图末尾会让"下游 Add 比它先出现"，`onnx.checker` 直接报拓扑序错误。

encoder 侧不用动：一次前向 1–2 ms（26 token 输入），全是 `DynamicQuantizeMatMul` / `MatMulIntegerToFloat`，没有同类问题。

**排查工具**：`scripts/mt/profile_ops.py`（算子剖面）、`probe_lmhead.py`（打印词表投影附近的图结构）、`probe_pattern.py`（打印 FC/attention 的正确 int8 写法当模板）、`transform_lmhead.py --check`（只报告会改什么）、`verify_lmhead.py`（数值 + 耗时 A/B）。

**实测**（本机，probe = "Failed to connect to the server. Please check your network settings."）

| 路径 | 耗时 | 译文 |
|---|---|---|
| `auto`（第 1 次） | 1243 ms | 无法连接到服务器。请检查您的网络设置。 |
| `auto`（第 2 次，google 已熔断） | 613 ms | — |
| `offline` | 667–766 ms | 连接到服务器失败。请检查您的网络设置。 |
| `mymemory` | 534–840 ms | 无法连接到服务器。请检查您的网络设置。 |
| `google` | 超时（本机网络不通） | — |

**配置持久化**：`%APPDATA%\com.shotglance.desktop\translate.json`，含密钥明文（仅本地，不进日志、不回显到错误信息）。

### 1.3 译文悬浮层（2026-11-13 实现）

框选 → 识别 → 自动翻译 → 卡片贴在原文框下方显示。核心约束仍是内存红线，所以**不新开窗口**。

| 决策 | 理由 |
|---|---|
| 面板窗口**变形**成卡片（摘边框 + 置顶 + `set_skip_taskbar`），不新建 overlay 窗口 | 多一个 WebView2 窗口 = 多 5–6 个子进程约 290 MB，与"空闲 < 80 MB"直接冲突。卡片只是几行字，不值得付这个代价 |
| 锚点用**屏幕物理坐标**（`RegionResult.screen_x/screen_y`） | 裁剪坐标是相对整屏帧的，少了显示器原点，多屏时卡片会一直贴在主屏左上角 |
| 卡片高度**两段式**：后端先给 96 CSS px 占位，前端量完内容再 `overlay_fit` | 文本长度未知，一次到位做不到。量的是 `.ov-text` 的 `scrollHeight`——卡片自身 overflow:hidden 且撑满窗口，其 scrollHeight 恒等于窗口高度 |
| 定位：框下方 → 上方 → 贴底 → 左右夹进该屏 | "贴底"是兜底：卡片比可用空间还高时，遮挡原文也好过被裁掉一半。`overlay_place` 有 5 个单测 |
| 悬浮期间空闲回收放宽到 **3 倍**（120 s → 360 s） | 读一句话用 2 分钟很正常；但仍要收——置顶无边框卡片赖着不走更烦人 |
| 热键唤回面板时先退悬浮 + `emit("overlay-dismiss")` | 否则"显示面板"只是把小卡片挪到眼前，用户要的是完整面板 |
| 自动翻译默认**开**，`Cfg.auto_translate` 可关 | 悬浮回显是主线体验；但 DeepL / OpenAI 按量计费，必须让用户能关。`serde(default)` 保证旧配置文件无缝升级 |

**交互**：`Esc` / `返回` 收卡片退回面板（译文仍留在面板里可复制）；`复制` 只复制译文；卡片显示期间按 `Ctrl+Shift+S` 直接回面板。

**实测**：样例锚点 `(420,300) 460×74` → 卡片 `476×96@(420,382)`（= 框底 374 + 8px），内容自适配后高 99 CSS px；安装包 21.89 MiB（+0.02）。

**取证方式**：`SHOTGLANCE_AUTO_OVERLAY=1` 打开 `index.html?auto=overlay` 摆样例卡片，再另起一进程 `SHOTGLANCE_SELF_TEST=1 SHOTGLANCE_DUMP_IMG=x.jpg` 存整屏图；几何另有日志 `[shotglance] 悬浮卡片 WxH+X,Y`。

## 2. 环境基线（本机实测版本，迁移时按此对齐）
| 组件 | 本机版本 | 安装来源 / 备注 |
|---|---|---|
| Windows | 10.0.22621.2428（Win11 22H2） | 新机器实测为 22631，无影响 |
| Rust | `1.99.1 (58cf8f9c3 2026-10-13)` stable **x86_64-pc-windows-msvc** | rustup-init，`%USERPROFILE%\.rustup`（2026-11-12 重装实测） |
| Cargo | `1.99.1 (5f94df478 2026-08-27)` | 同上 |
| MSVC Build Tools | VS2022 BuildTools `17.14.37906.3`，MSVC `14.44.35207` | **装在 `Program Files (x86)`**；必须勾 `VC.Tools.x86.x64` |
| Windows SDK | `10.0.26100.0` | 随 Build Tools（比旧基线 22621 新，兼容） |
| Node.js | `v22.22.2` | 要求 ≥ 20.19（Vite 7） |
| npm | `10.9.7` | — |
| Python | `3.13.12`（托管版）/ `3.13.14`（旧机） | 仅用于资产校验/小工具，不参与构建 |
| WebView2 Runtime | `152.0.4191.62` / `157.0.4341.46` | 系统自带；Tauri 2 要求 ≥ 92 |
| NSIS | 3.13（缓存于 `%LOCALAPPDATA%\tauri\NSIS`） | 断网时用 `scripts\restore-nsis-cache.ps1` 从 vendor 恢复 |
| ONNX Runtime | 1.31.1（`onnxruntime.dll` 19288888 B） | 从 PyPI 清华镜像的 wheel 里解出 |

Rust 工具链只需 `stable-msvc`，**不需要** nightly、不需要 `+crt-static`（Rust 1.99 默认已静态链接 vcruntime）。

> 判断 MSVC 到底装没装，**只用 vswhere**，别按路径猜（Build Tools 装在 `Program Files (x86)`，
> 按 `Program Files` 找会得出"没装上"的错误结论）：
> ```powershell
> & 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' -products * `
>   -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
> ```

---

## 3. 本次保存包含的文件与配置范围

快照用**显式白名单**生成（`scripts/export-snapshot.ps1`），不是通配符拷贝，所以任何几 GB 的缓存都不会被误带。
代价是**新增文件必须手动加进白名单**——2026-11-09 那版就因为漏了 `translate/` 整个模块，快照里没有翻译层。改代码时顺手改一下脚本里的 `$IncludeFiles`。

当前快照：55 个文件（含 `MANIFEST.txt`），原始 59.79 MB，压缩后 45.33 MB。

### 3.1 包含

| 分组 | 路径 | 体积 | 说明 |
|---|---|---|---|
| 工作区 | `README.md`（上级目录）、`verify-env.ps1` | ~10 KB | 索引 + 8 项环境自检 |
| 前端 | `index.html`、`src/main.ts`、`src/styles.css`、`vite.config.ts`、`tsconfig.json`、`package.json`、`package-lock.json` | ~100 KB | 无框架原生 TS，940 行 |
| 后端 | `src-tauri/src/{lib.rs,main.rs,capture/mod.rs,ocr/{mod,det,rec}.rs,translate/{mod,online,offline}.rs}` | 145 KB | 全部业务逻辑，约 3600 行 |
| 配置 | `src-tauri/Cargo.toml`、**`Cargo.lock`**、`build.rs`、`tauri.conf.json`、`.cargo/config.toml`、`capabilities/`、`icons/` | ~380 KB | `Cargo.lock` 必须带：`ort` 锁在 `=2.0.0-rc.15`，API 与教程版本差异极大 |
| OCR 模型 | `src-tauri/models/{det.onnx,rec.onnx,dict.txt}` | 15 MB | PP-OCRv4，随包分发 |
| OCR 运行时 | `src-tauri/vendor/onnxruntime.dll` | 19 MB | ORT 1.31.1，不在系统 PATH 上 |
| 翻译词表 | `src-tauri/assets/mt/tokens.tsv` | 1.8 MB | 65001 行 `id/score/piece`，与模型版本绑死，随包分发 |
| 脚本 | `scripts/{measure-memory,restore-nsis-cache,make-ocr-sample,export-snapshot}.ps1`、`scripts/gen_icon.py`、`scripts/mt/{ref,cmp,export_vocab,merge_vocab,bench_decode,profile_ops,probe_lmhead,probe_pattern,transform_lmhead,verify_lmhead}.py` + `scripts/mt/README.md`、`scripts/vendor/`（NSIS 3.13 + 插件 dll） | 2.4 MB | NSIS vendor 与 mt 转换脚本都是"重建很贵"的东西，必须带 |
| 基准 | `src-tauri/tests/ocr-sample.png` | 24 KB | OCR 可复现验证图 |
| 文档 | `README.md`、本文件、`.workbuddy/memory/*` | 90 KB | 决策依据 + 踩坑记录 + 记忆 |
| 成品（可选） | `release/ShotGlance_0.1.0_x64-setup.exe` | 22 MB | 不想装工具链时直接安装。`-NoInstaller` 可跳过 |

**离线翻译模型（113 MB）不在快照里**，也不该在：它只对选了离线引擎的用户有价值，放进去会把 45 MB 变成 160 MB。
首次使用在应用内下载到 `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`，或者从旧机器直接拷这个目录。

### 3.2 排除（可重建，不进快照）

| 路径 | 体积 | 重建方式 |
|---|---|---|
| `shotglance/src-tauri/target` | 7.6 GB | `npm run tauri:build` |
| `shotglance/node_modules` | 62 MB | `npm ci` |
| `shotglance/src-tauri/gen` | 313 KB | `tauri-build` 自动生成 |
| `shotglance/dist` | 204 KB | `npm run build` |
| `.setup`（rustup/vs_BuildTools/nsis 安装包） | 19 MB | 官方渠道重新下载；NSIS 那份已另存 `scripts/vendor/` |
| `NVIDIA Corporation/`（根目录与 `src-tauri/` 各一份） | 0 | 显卡驱动安装器留下的空目录，2026-11-11 已删 |

### 3.3 资产校验和（到新机器上比一遍）

```
d2a7720d45a54257…  4745517  models/det.onnx
48fc40f24f6d2a20…  10857958 models/rec.onnx
a1c84d9bdb9ab290…  26250    models/dict.txt
68d0d5f502be4509…  19288888 vendor/onnxruntime.dll
e50ef81b726f7d68…  1867691  assets/mt/tokens.tsv
```


完整逐文件 SHA256 在压缩包内的 `MANIFEST.txt`，同目录还有 `*-report.txt`（包含/排除清单）。

---

## 4. 目标设备恢复步骤

> **先看这一段，可能省你 3 小时。**
> 如果手上有 `env-bundle\`（由 `scripts/export-env-bundle.ps1` 导出），
> **不要走下面的 Step 0–3**，直接跳到 [`ENV-MIGRATION.md`](ENV-MIGRATION.md)：
>
> ```powershell
> cd <工作区>\shotglance
> powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -AddToPath -WithVsInstall -Check
> ```
>
> 它一次完成工具链恢复 + 依赖缓存 + node_modules + MSVC 离线安装 + 自检，实测 15–20 分钟。
> 下面的步骤是**没有环境包时**的联网路径，2026-11-12 实测耗时约 3 小时（其中 2.5 小时是下载）。

### Step 0 — 装工具链（只做一次）

1. **VS2022 Build Tools**：勾 *使用 C++ 的桌面开发*，确认明细里有
   `Microsoft.VisualStudio.Component.VC.Tools.x86.x64` 和 Windows 10/11 SDK。
   （用 `vs_BuildTools.exe` 引导安装，winget 静默安装实测返回 0 但什么都没装。）
   > 命令行装用这套参数（实测能编过整个 Tauri 依赖树）：
   > `--add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --includeRecommended --quiet --wait --norestart`
2. **Rust**：`rustup-init.exe`，选 `stable-x86_64-pc-windows-msvc`，默认 host。
   实测 34 分钟（71 KB/s），国内镜像**没有** `rustup-init` 这个文件，别指望加速。
3. **Node** ≥ 20.19。
4. **WebView2 Runtime**：Win11 自带；Win10 用安装包里的 bootstrapper 或系统安装。

> 装 VS 前先确认环境变量里没有重复的 `HTTP_PROXY` + `http_proxy` —— 会触发安装器 `0x80070057`，
> 而且**是在启动任何东西之前就退出**。
> 命令行走 `Start-Process -Verb RunAs` 时子进程会继承父环境块，`env -u` 那套不管用；
> 必须在**发起提权的那一个进程**里先清：
> ```powershell
> foreach ($n in @('http_proxy','https_proxy','all_proxy','no_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY')) {
>   [Environment]::SetEnvironmentVariable($n, $null, 'Process')
> }
> ```
> 另：`Start-Process -Verb RunAs` **必须加 `-Wait`**，否则调用返回即被回收（本次踩过，日志精确停在返回时刻）。

### Step 1 — 解压并校验

> **路径尽量与源机器一致**（本次是 `C:\Users\Administrator\Downloads\2026-11-09-08-43-20`）。
> 只有路径一致，`env-bundle\target` 才能复用（Rust 的 dep-info 里存的是绝对路径）。
> 做不到就用固定短路径，比如 `C:\dev\shotglance`，以后不要再动。

```powershell
Expand-Archive shotglance-snapshot-*.zip -DestinationPath C:\work\shotglance-restore
cd C:\work\shotglance-restore
Get-FileHash shotglance\src-tauri\vendor\onnxruntime.dll -Algorithm SHA256   # 对比 3.3
```

### Step 2 — 环境自检

```powershell
powershell -ExecutionPolicy Bypass -File .\verify-env.ps1
```

### Step 3 — 装前端依赖

```powershell
cd shotglance
npm ci            # 必须用 ci，保证与 package-lock.json 一致
```

### Step 4 — 开发模式验证（改前端热重载）

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"   # tauri CLI 要能找到 cargo
npm run tauri:dev
```

托盘出现 → 按 `Ctrl+Shift+S` 出面板 → 点「识别」应出现文本。**这一步成功就说明 dll、模型、路径全部正确。**

### Step 5 — 无界面自检（脚本化 / CI 用，不需要点界面）

```powershell
cd src-tauri
# 截图：冷/热多轮 + 编码对比
$env:SHOTGLANCE_SELF_TEST='1'; .\target\release\shotglance.exe
Get-Content "$env:TEMP\shotglance-selftest.txt"

# OCR：可复现基准图
$env:SHOTGLANCE_OCR_IMG='tests/ocr-sample.png'
$env:SHOTGLANCE_OCR_SELFTEST='1'; .\target\release\shotglance.exe
Get-Content "$env:TEMP\shotglance-ocr-selftest.txt"     # 期望 8 行、conf > 0.95
```

### Step 6 — 打包

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cd C:\work\shotglance-restore\shotglance
npm run tauri:build
```

断网或 NSIS 缓存被清空时，打包器会去 GitHub 拉 NSIS 并卡死（本机 github.com 走代理 502），先跑：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\restore-nsis-cache.ps1
```

### 恢复后应看到的基线

| 检查点 | 预期 |
|---|---|
| `verify-env.ps1` | 8 项全绿 |
| 托盘空闲内存 | 11–33 MB |
| 面板回收后内存 | 23–33 MB（`SHOTGLANCE_WARMUP=0` 可回到 23.7 MB） |
| 首次截图 | 130 ms 级（冷启动首次 351 ms 属正常） |
| 基准图 OCR | 8/8 行，置信 0.97–1.00 |
| 安装包 | 22 MB 左右 |

---

## 5. 不可回退的坑（踩过，别重踩）

| 坑 | 症状 | 处理 |
|---|---|---|
| Tauri 2 的 dev/prod 开关 | 手编的 exe 双击弹「无法访问此页面」，去连 `localhost:1420` | 手编必须 `--features tauri/custom-protocol`；正常走 `npm run tauri:build` |
| `cdn.pyke.io` 被限速 | 只有 ~2 KB/s，`ort-sys` build script 静默挂死，无日志无进程 | 已改用 PyPI 镜像 wheel + `load-dynamic`；不要回退到 `download-binaries` |
| `ort` API 版本差异 | rc.15 与常见教程完全不同：`Session::builder(&Environment)`、`run(&mut self)`、`Tensor::from_array((shape, Vec))` | `Cargo.toml` 锁 `=2.0.0-rc.15`，注释里写了签名 |
| `SessionOutputs` 借用 | 输出借着 `&mut self.det`，直接再借 self 跑识别编译失败 | 用 block 让输出先出作用域 |
| 字典类别数 | 英文单词全粘一起 | 6625 类 = 1 blank + 6623 字典 + 1 空格，空格必须挂最末类；不要在解析时 trim |
| PowerShell 编码 | `,` 后面缺少表达式；参数被静默丢弃 | `.ps1` 要么纯 ASCII，要么 UTF-8 **带 BOM**。PS 5.1 按 ANSI 读无 BOM UTF-8 |
| PowerShell 执行策略 | 脚本一律拒绝运行 | `Set-ExecutionPolicy -Scope Process Bypass -Force` 或 `-ExecutionPolicy Bypass` |
| 给 Rust 传 POSIX 路径 | `/tmp/x.jpg` 落到 `C:\tmp`，目录不存在则 `fs::write` 静默失败 | 用 `cygpath -w` 转 Windows 路径 |
| Vite `emptyOutDir` | 构建被沙箱 safe-delete 拦截（累计删除计数阈值 50） | `vite.config.ts` 已设 `emptyOutDir: false`，别改回去 |
| `hide()` 不省内存 | 关窗后内存不降 | 一律 `destroy()`；WebView2 子进程退出要 4–8 s，测内存必须轮询 |
| xcap D3D 缓存 | 采集后常驻 +42 MB 且不可释放 | 非泄漏（`LazyLock` 只初始化一次）；`SHOTGLANCE_WARMUP=0` 可省，代价是首次截图 351 ms |
| `Monitor::all()` 副作用 | 只打开面板就付 42 MB | 前端首次截图成功后才加载显示器列表 |
| Tauri identifier 以 `.app` 结尾 | 与 macOS bundle 扩展名冲突 | 已改为 `com.shotglance.desktop` |
| 远程桌面窗口被遮挡 | 建窗成功但看不见，误判「打不开」 | 取证加 `SHOTGLANCE_TOPMOST=1` |
| `set_resizable(false)` 夹住 `set_size` | 窗口量到 2560×1440，屏幕上还是 520×460 | Windows 上它会给窗口写 min/max track size，随后的 resize 被 `WM_GETMINMAXINFO` 夹回。全屏窗口别禁用缩放 |
| DWM 不可见边框 | 客户区 2560×1440 对应外框 2576×1449，按外框原点摆窗口会整体偏 8×1 px | 用 `inner_position() - outer_position()` 反算偏移后再 `set_position` |
| 窗口 resize 后 WebView2 不重绘 | 尺寸已变，但遮罩底色/底图不画，屏幕上留旧帧 | 先 resize、再显示遮罩层（两次 rAF 之后），别在 resize 前就把全屏层挂上 |
| `ureq` 的 `native-tls` 不生效 | 所有 https 请求报 `no TLS backend is configured`，看着像网络问题 | ureq 只把开启 `tls`(rustls) feature 的后端当默认，`native-tls` feature 仅"可用"。必须在 `AgentBuilder` 上调 `.tls_connector(Arc::new(native_tls::TlsConnector::builder().build()?))` 显式注入 |
| `mod translate` 与 `fn translate` 同名 | `translate::run` 被解析成函数路径，25 个 E0433 | 模块用 `#[path = "translate/mod.rs"] mod mt;`，命令名保持 `translate` |
| Marian 的 spm 与 vocab 是两套 id | 直接用 spm 的 id 喂模型，输出全是乱码 | `source.spm` 只有 32000 个 piece，embedding 是 65001 行；必须拿 piece 字符串过 `vocab.json` 映射。构建期合成 `tokens.tsv` 一次解决 |
| SentencePiece 的 `unk` 分数 | 自己实现 Viterbi 时整句输出 unk | `unk_score` 要取 `min_score - 10`；spm 报的 unk 分数是 0，直接用它会让 unk 路径得分最高 |
| 目标语言中文的词间空格 | 译文里每个词之间都有空格（`连接到服务器失败。 请检查`） | spm 解码后要把 `▁` 还原成空格，但中文之间不该有空格；`polish()` 里删中文→空格、空格→中文标点、中文旁半角标点转全角 |
| merged decoder 的 `use_cache_branch` | 少喂一个输入直接报 `unknown input`；恒定置真则 Reshape 崩 | 首步假、之后真。它决定 "重算整段" 还是 "只算新 token + 读 cache" |
| past 的 0 长度维 | `If` 子图里报 `The dimension with value zero exceeds the dimension size of the input tensor` | ORT 的 Reshape 不接受 seq=0。首步喂 `[1,8,0,64]` 空张量并让 `use_cache_branch=0`，别塞零值占位 |
| `present.*.encoder.*` 形状坏 | 前两步正常，第 3 步必崩 | cache 分支给出的 cross-attention present 是 `[0,8,1,64]`。encoder 侧 K/V 全程不变，只收首步的 present |
| `If` 子图里的常量不折叠 | 词表投影每步重新转置 33 MB、展开成 133 MB fp32，单步白烧 12 ms | ORT 的常量折叠**不跨 `If` 子图**。改成预转置 int8 + `MatMulInteger`，见 §1.5 |
| protobuf 节点不能按 `id()` 比 | `n is matmul` 恒为假，图改造静默不生效，`onnx.checker` 报拓扑序错 | repeated 容器每次访问都可能返回新 wrapper。改图一律按 `node.name` 匹配 |
| 图改造后节点顺序 | `onnx.checker` 报 `Nodes in a graph must be topologically sorted` | 新节点必须插在被替换节点**原来的位置**；append 到末尾会被更早的下游节点引用 |

---

## 6. 未决事项

### 6.1 下一步路线（已定顺序）

1. ~~**译文悬浮层**~~ ✅ 2026-11-13 完成，见 §1.3。
2. ~~**OCR 后自动翻译**~~ ✅ 2026-11-13 完成，做成了设置里的开关（`Cfg.auto_translate`，默认开）。
3. ~~**离线引擎解码提速**~~ ✅ 2026-11-16 完成，分两步：
   - merged decoder + KV cache（§1.4）：短句 1.2×、长句 1.6×+。当时判断"要继续压只能动量化/算子层"。
   - 词表投影 int8 化（§1.5）：算子剖面定位到 `If` 子图里每步重算的 `Transpose + DequantizeLinear`，改成 int8 直算后**单步 14.84 → 2.37 ms（6.3×）**，累计对原始实现 6.1–6.8×。三句基准译文逐字不变。
4. **识别小批优化** — 同批 padding 到最大宽，一次前向；可压掉逐行调度的开销。区域框选落地后全屏场景已不常见，优先级下调。
5. **Decoder 布局算子收敛**（新）— §1.5 之后单步 2.4 ms 里，KV cache 的 `Concat`(109 次)/`Reshape`(98 次)/`Unsqueeze`(50 次) 占三成。要压得改图结构（例如只导出 decoder 侧 past、砍掉坏掉的 `present.encoder.*`），收益两位数百分比，风险中等。
6. **悬浮卡片样式打磨**（可选）— 目前窗口不透明，卡片是方角 + 内描边。真圆角/投影需要建窗时开 `transparent(true)`，或改用原生 Win32 分层窗口（GDI+ 逐像素 alpha），代价与风险都比现在高，暂不做。

### 6.2 待确认

- **跨平台**：目前只在 Windows 验证过。macOS 需要 `WKWebView` + 屏幕录制权限（首次采集要用户授权），Linux 需要 `webkit2gtk`（无法完全自包含）。要上这两端，得先确认优先级。
- ~~用户称呼 / 我的称呼~~ → 2026-11-16 已落定（我叫「镜」），`SOUL.md` / `IDENTITY.md` / `USER.md` 已写，别再每轮重问。

### 6.3 已知技术债

- 离线翻译模型不随包分发，首次使用需下载（原版 113 MB；跑过 `transform_lmhead.py` 后 146 MB，多出的 32 MB 换 6 倍单步速度）。
- `decoder.int8.onnx` 需要手工跑 Python 脚本生成——Rust 侧只做"探测有就用"，不做自动转换。要做成全自动，得在 Rust 里操作 ONNX protobuf，成本远高于收益。
- 识别在长文本下会丢词间空格（模型层问题，可后处理补救）。

---

## 7. 文件地图

```
工作区\
├─ README.md              工作区索引（换设备先读这一页）
├─ verify-env.ps1         8 项环境自检
├─ shotglance-snapshot-*.zip / *-report.txt
└─ shotglance\
   ├─ index.html           面板骨架（预览 + 原文 + 译文三区）
   ├─ HANDOFF.md  README.md
   ├─ release\             已构建的安装包（可直接分发，22 MB）
   ├─ src\
   │  ├─ main.ts           原生 TS，无框架；invoke 通道 + 自动识别钩子
   │  └─ styles.css
   ├─ src-tauri\
   │  ├─ src\
   │  │  ├─ lib.rs         托盘、热键、建窗/销毁、watchdog、选区形态切换、命令注册、自检入口
   │  │  ├─ main.rs        入口
   │  │  ├─ capture\mod.rs xcap 采集、区域裁剪、JPEG 编码、预热、显示器枚举
   │  │  ├─ ocr\
   │  │  │  ├─ mod.rs      ort 会话管理、模型寻址、加载/卸载、推理编排
   │  │  │  ├─ det.rs      DB 检测前处理 + 连通域后处理 + 阅读顺序
   │  │  │  └─ rec.rs      CRNN 前处理 + CTC 贪心解码
   │  │  └─ translate\     翻译层（Rust 模块名 mt，见 §5 同名冲突）
   │  │     ├─ mod.rs      配置持久化、引擎注册表、缓存、分段、对冲竞速、失败熔断
   │  │     ├─ online.rs   DeepL / OpenAI 兼容 / Google / MyMemory 四个 provider
   │  │     └─ offline.rs  词表 + Unigram Viterbi + ONNX encoder-decoder 贪婪解码 + 模型下载
   │  ├─ models\           PP-OCRv4 det/rec + 6623 字字典（15 MB）
   │  ├─ assets\mt\        tokens.tsv（65001 行 id/score/piece，1.8 MB，随包分发）
   │  ├─ vendor\           onnxruntime.dll（19 MB）
   │  ├─ tests\            ocr-sample.png（OCR 基准图）
   │  ├─ capabilities\     权限白名单
   │  └─ tauri.conf.json   空窗口列表、resources、NSIS 配置
   └─ scripts\
      ├─ export-snapshot.ps1    白名单打包（新增文件记得加进 $IncludeFiles）
      ├─ restore-nsis-cache.ps1 离线恢复 NSIS（本地优先 → ghproxy 回退 + SHA256）
      ├─ measure-memory.ps1     内存三阶段实测
      ├─ make-ocr-sample.ps1    生成 OCR 基准图（GDI+）
      ├─ gen_icon.py            重生成图标（纯 stdlib，无 PIL）
      ├─ mt\                    离线翻译模型转换脚本（ref/cmp/export_vocab/merge_vocab）
      └─ vendor\                nsis-3.13.zip + nsis_tauri_utils.dll
```

---

## 8. 环境变量速查（全部可选，不影响默认行为）

| 变量 | 作用 |
|---|---|
| `SHOTGLANCE_MODELS` | 覆盖模型目录 |
| `SHOTGLANCE_ORT_DLL` | 覆盖 onnxruntime.dll 路径 |
| `SHOTGLANCE_ORT_THREADS` | 覆盖 intra 线程数（默认 `min(核数,4)`） |
| `SHOTGLANCE_IDLE_SECS` | 空闲回收阈值（默认 120 s） |
| `SHOTGLANCE_WARMUP=0` | 关闭采集预热（省 42 MB，首次截图变慢） |
| `SHOTGLANCE_SELF_TEST=1` | 无界面截图自检 |
| `SHOTGLANCE_OCR_SELFTEST=1` | 无界面 OCR 自检 |
| `SHOTGLANCE_OCR_IMG` | 用图片代替截图（可复现） |
| `SHOTGLANCE_CROP` | 自检时按 `x,y,w,h` 裁一刀再识别，验证裁剪取整与越界夹取 |
| `SHOTGLANCE_HOLD_SECS` | 自检后保持进程 N 秒，供外部采样内存 |
| `SHOTGLANCE_UNLOAD_TEST=1` | 保持前先卸载引擎，验证回到基线 |
| `SHOTGLANCE_OPEN_PANEL=1` | 启动即建面板（自动化用） |
| `SHOTGLANCE_TOPMOST=1` | 面板置顶（取证用） |
| `SHOTGLANCE_AUTO_OCR=1` | 面板加载后自动跑一次识别 |
| `SHOTGLANCE_DUMP_IMG` | 自检时把采集帧落盘 |
| `SHOTGLANCE_MT_SELFTEST=1` | 无界面翻译自检：auto 两轮 + 逐引擎，落盘后退出 |
| `SHOTGLANCE_MT_TEXT` | 自检用的 probe 文本（默认是一句英文错误提示） |
| `SHOTGLANCE_MT_DIR` | 覆盖离线模型目录（默认 `%APPDATA%\ShotGlance\models\opus-mt-en-zh`） |

dll 寻址顺序：`SHOTGLANCE_ORT_DLL` → exe 同级 → `exe/models/` → 仓库 `vendor/`。

---

## 9. 2026-11-11 换设备前的整理记录

做过的（目的是让工作区"拷走即用"，体积从 7.8 GB 降到约 100 MB）：

| 动作 | 内容 |
|---|---|
| 清理 | 删 `NVIDIA Corporation/`（根目录与 `src-tauri/` 各一个空目录，显卡驱动安装器留下的）；删 `.setup/` 里的安装器（`gen_icon.py` 已移到 `scripts/`，NSIS 那两个文件 `scripts/vendor/` 本来就有） |
| 归档 | 把 `target/release/bundle/nsis/` 的安装包复制到 **`shotglance\release\`**，这样清掉 `target` 之后成品还在 |
| 重建 | 重新生成快照（旧快照是 11-09 的，不含翻译层）；`export-snapshot.ps1` 白名单补上 `translate/*`、`assets/mt/tokens.tsv`、`scripts/mt/*`、`scripts/gen_icon.py`、根 `README.md` |
| 文档 | 新增工作区根 `README.md`（索引 + 三条恢复路径 + 校验和）；本文件 §3 按实际内容重写；修 `verify-env.ps1` 里 "PP-OCRv6" 的笔误 |

没做的，以及原因：

- **没有 `git init`**。这是最后一件该做但没做的事——`Cargo.lock`、`gen/`、`tauri.conf.json` 目前没有任何版本控制兜底。新设备上第一件事建议 `git init` 并把 `node_modules/ target/ dist/ gen/` 写进 `.gitignore`（已有基础版）。
- **没有删 `node_modules` / `target`**（本次保留，方便在旧机器上继续验证）。它们不进快照，删不删都不影响迁移，只是占盘。
- **没有动 `dist/` 与 `gen/`**：都是构建期自动重生成的，删了也不影响，但 `tauri:dev` 又要多等一轮。

### 迁移时最容易漏的两样东西

1. `src-tauri/assets/mt/tokens.tsv`（1.8 MB）。漏了它在线翻译照常工作，**只有离线引擎会在加载时报词表缺失**，很容易到了新机器才发现。
2. `scripts/vendor/`（NSIS 3.13 + 插件 dll）。漏了它，`tauri build` 会去 GitHub 拉 NSIS，在受限网络下卡死。

