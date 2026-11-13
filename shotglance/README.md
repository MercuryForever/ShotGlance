# ShotGlance

截图 → OCR → 翻译 → 悬浮回显。Rust + Tauri 2 技术栈。

> 换机器 / 隔很久回来 → 先看 [`HANDOFF.md`](HANDOFF.md)（环境基线、快照范围、恢复步骤、踩坑清单）。
> 本文只讲技术决策与实测数据。

## 环境要求

| 组件 | 版本 | 用途 |
|---|---|---|
| Rust | stable-x86_64-pc-windows-msvc (1.99+) | 核心逻辑 |
| MSVC Build Tools | VS2022 VCTools + Win11 SDK | 链接器与系统库 |
| Node.js | ≥ 20.19 | 前端构建与 Tauri CLI |
| WebView2 Runtime | 系统自带 | Tauri 渲染引擎 |

自检：`powershell -ExecutionPolicy Bypass -File ..\verify-env.ps1`

## 开发

```bash
npm install
npm run tauri:dev      # 热重载，改前端秒级生效
```

## 构建

```bash
set PATH=%USERPROFILE%\.cargo\bin;%PATH%   # tauri CLI 需要能找到 cargo
npm run tauri:build
# 产物: src-tauri/target/release/bundle/nsis/ShotGlance_0.1.0_x64-setup.exe（21.91 MiB）
```

> **只跑 `cargo build --release` 会得到一个「开发模式」的 exe**：Tauri 2 用
> `tauri/custom-protocol` feature 区分开发/生产，缺了它 exe 仍然去连
> `http://localhost:1420`，双击后弹 "无法访问此页面"。手动编译请用
> `cargo build --release --features tauri/custom-protocol`，正常发布走
> `npm run tauri:build` 即可（CLI 会自动带上）。

发布前检查清单：
1. `dumpbin /dependents` 确认无 `VCRUNTIME140.dll`（Rust 1.99 默认已静态链接 vcruntime，实测无需 `+crt-static`；该开关只在需要支持 Win7/8 时再打开）
2. 在 Windows Sandbox 里断网跑一遍
3. 确认 NSIS 缓存完整（见下节），否则打包会尝试联网

## 离线打包（网络受限环境）

`cargo tauri build` 会从 `github.com` 下载 NSIS 打包工具。若 `github.com` 不可达
（`CONNECT proxy failed: 502`），打包会在最后一步失败。用项目自带的脚本预热缓存：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\restore-nsis-cache.ps1
```

优先级顺序：本地 `scripts/vendor/` → GitHub 镜像（ghproxy.net）。
两个文件都做 SHA256 校验，成功后写入 `%LOCALAPPDATA%\tauri\NSIS\`，之后打包全程不联网。

| 资产 | 大小 | 用途 |
|---|---|---|
| `vendor/nsis-3.13.zip` | 2.3 MB | NSIS 编译器本体（441 文件） |
| `vendor/nsis_tauri_utils.dll` | 34 KB | Tauri 的 NSIS 插件，v0.5.3 |

注意插件必须存在于两处：`Plugins/x86-unicode/additional/`（bundler 校验路径）
和 `Plugins/x86-unicode/`（makensis 加载路径）。只放一处会被判定"缺少文件"并整目录重建。

> ExecutionPolicy 为 `Restricted` 时脚本会被拒绝执行，必须带 `-ExecutionPolicy Bypass`。
> 也可用 `Set-ExecutionPolicy -Scope Process Bypass -Force` 只对当前会话放开。

## 网络依赖清单

| 阶段 | 需要的域名 | 说明 |
|---|---|---|
| `cargo fetch` / 编译 | crates.io、static.crates.io | 已缓存则离线可用 |
| `npm install` | registry.npmjs.org | 同上 |
| `tauri build` → NSIS | github.com | **唯一阻塞点**，用上方脚本绕开 |
| `tauri build` → WebView2 bootstrapper | go.microsoft.com | 仅 `embedBootstrapper` 模式需要 |
| OCR 模型 | hf-mirror.com | PP-OCRv4 det/rec，直连可用（huggingface.co 不可达） |
| 字典文件 | ghproxy.net → raw.githubusercontent.com | 2 万字节，随仓库提交 |
| ONNX Runtime dll | pypi.tuna.tsinghua.edu.cn | **cdn.pyke.io 实测被限速到 ~2 KB/s**，改用 PyPI 镜像 |

## 窗口生命周期（内存指标的关键设计）

**启动时零窗口**（`tauri.conf.json` 的 `app.windows` 为空数组），进程只驻留托盘。
面板按需创建、用完销毁：

| 触发 | 行为 |
|---|---|
| `Ctrl+Shift+S` / 托盘左键 | 建窗（已存在则前置复用） |
| Esc / 关闭按钮 | **destroy** 窗口，不是 hide |
| 无活动 120 秒 | watchdog 自动 destroy |
| 托盘 → 退出 | `app.exit(0)` |

为什么必须 destroy 而不是 hide：窗口"隐藏"时 WebView2 的子进程照常存活，
内存一分不省。实测数据（`scripts/measure-memory.ps1`）：

| 阶段 | 主进程 | WebView2 子进程 | 合计 |
|---|---|---|---|
| 常驻（从未建窗） | 11.5 MB | 0 个 / 0 MB | **11.5 MB** |
| 面板打开（已预热采集） | 64.7 MB | 6 个 / 265.1 MB | 329.8 MB |
| 空闲回收后 | 64.8 MB | 0 个 / 0 MB | **64.8 MB** |

两个必须知道的时间与内存细节：

1. **窗口销毁后 WebView2 不立即退出**，实测要 4–8 秒才归零（10 s 时还是 6 个进程，
   14 s 才 2 个，18 s 归零）。所以看任务管理器"按了 Esc 怎么还有进程"是正常的；
   验收脚本也必须轮询等待，不能用固定 sleep 采样，否则会误报失败。
2. **回收后是 65 MB 还是 23 MB，取决于有没有用过截图**。D3D 设备初始化后不可释放
   （见「截屏采集 → 内存代价」）。从不截图就是 23.7 MB。

所以"空闲 < 80 MB"这个指标，压模型或调线程池都做不到，**只有回收窗口能做到**。
回收的代价是前端状态丢失（DOM、滚动位置），重建约 100–300 ms；120 秒的宽限期
是为了让"看一眼原文再回来看译文"这种操作不会误触发回收。

两个可测性开关（不改变默认行为）：

| 环境变量 | 作用 |
|---|---|
| `SHOTGLANCE_OPEN_PANEL=1` | 启动即建面板，用于脚本化测量窗口态 |
| `SHOTGLANCE_IDLE_SECS=n` | 覆盖空闲阈值，便于十几秒内验证回收 |

验证：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\measure-memory.ps1 -IdleSecs 10
# 三项 PASS + failures: 0 才算通过
```

## 截屏采集

用 `xcap`（Windows 下开 `wgc` feature → Windows.Graphics.Capture），不手写三套原生调用。

| 决策 | 选择 | 理由 |
|---|---|---|
| 采集库 | xcap 0.9 + `wgc` | 三平台同一套 API，省掉三套平台分支与各自回归；代价是 windows crate，包体只涨约 40 KB |
| 预览编码 | JPEG q85 | 实测 PNG 126 ms / 888 KB vs JPEG 75 ms / 364 KB。预览是给人看的，无损没有意义 |
| OCR 输入 | 内存原始 RGBA | 不经过编码，因此不受 JPEG 有损影响 |
| 预热 | 建窗时后台预热 | 首次采集 256–273 ms（初始化 D3D11 与 WGC 帧池），稳态仅 35–53 ms |

实测（2560×1440，单显示器）：

| 场景 | 采集 | 编码 | 合计 | 判定 |
|---|---|---|---|---|
| 冷启动首次（无预热） | 256 ms | 75 ms | 331 ms | ❌ 超 300 ms |
| 预热后（真实路径） | 35–53 ms | 76–86 ms | **121–130 ms** | ✅ 约 2.3 倍余量 |

注意瓶颈不在采集，而在编码与首次初始化——这跟直觉相反，也是必须分段打点的原因。
上表的"合计"不含 IPC 与前端解码，界面上的"往返"数字才是完整的用户时延。

### 内存代价（预热换时延）

xcap 的 Windows 实现用 `LazyLock` 全局单例缓存 `ID3D11Device`、`ID3D11DeviceContext`、
`IDXGIDevice` 和按显示器索引的 `GraphicsCaptureItem`，**没有 clear API**。因此：

- 只要真正采集过一次，常驻内存 23 MB → **65 MB**，进程退出前不归还；
- 但只初始化一次，**不随截图次数累积**，不是内存泄漏；
- 收益是首次采集 351 ms → 130 ms。

另外，**枚举显示器也会初始化这套设备**。所以前端不做启动时预枚举，
显示器下拉列表在首次截图成功后才填充——否则用户只是打开面板看一眼，
也会白白付掉那 42 MB。

按机器自行取舍，两种模式实测都在硬指标内：

| 模式 | 首次采集 | 常驻（回收后） | 启用方式 |
|---|---|---|---|
| 预热（默认） | **130 ms** | 64.8 MB | 默认 |
| 关闭预热 | 351 ms ⚠️ | **23.7 MB** | `SHOTGLANCE_WARMUP=0` |

```powershell
# 两种模式都能用同一脚本复核（决策依据，不是估计值）
powershell -ExecutionPolicy Bypass -File .\scripts\measure-memory.ps1 -IdleSecs 10
powershell -ExecutionPolicy Bypass -File .\scripts\measure-memory.ps1 -IdleSecs 10 -NoWarmup
```

多屏 / 4K / 不同显卡驱动下 D3D 设备占用会更大，建议在目标机器上复核 80 MB 是否
仍成立。若超了，先关预热。

无界面自检（结果写入 `%TEMP%\shotglance-selftest.txt`）：

```powershell
$env:SHOTGLANCE_SELF_TEST='1'; .\src-tauri\target\release\shotglance.exe
# 输出冷启动基线 + 预热后三轮，以及 PNG/JPEG 编码对比
```

## OCR（PP-OCRv4 + ONNX Runtime）

不引 PaddleOCR 的 C++ 推理库：包体会从 20 MB 涨到 300 MB 以上，还要随包带整套 Paddle 依赖。
ONNX Runtime + PP-OCRv4 移动端模型能把整条 OCR 链路压在 35 MB 以内。

| 资产 | 大小 | 说明 |
|---|---|---|
| `models/det.onnx` | 4.5 MB | PP-OCRv4 检测（DB） |
| `models/rec.onnx` | 10.4 MB | PP-OCRv4 识别（CRNN + CTC） |
| `models/dict.txt` | 26 KB | 6623 字字典，加 blank 与空格共 6625 类 |
| `vendor/onnxruntime.dll` | 18.4 MB | ORT 1.31.1，随安装包放 exe 同级 |
| `shotglance.exe` | 3.7 MB | |

### 为什么用 load-dynamic 而不是 download-binaries

`ort` 默认的 `download-binaries` 会去 `cdn.pyke.io` 拉预编译包，但本机实测被限速到
**~2 KB/s**（32 MB 要几小时，构建脚本会一直挂在那里）。改从 PyPI 镜像取同版本的
`onnxruntime.dll`（`onnxruntime-1.31.1` wheel，14888304 字节，SHA256
`0558f29d…c2348b`），与 `ort-sys` 期望的 ORT API 1.31 对齐。

wheel 里只有 dll、没有 `onnxruntime.lib`，链接期本来就无法静态绑定——这正好是
`load-dynamic` 的适用场景：运行时用 `libloading` 打开 dll，位置由我们自己控制，
安装后不依赖系统 PATH。

dll 寻址顺序：`SHOTGLANCE_ORT_DLL` → exe 同级 → `exe/models/` → 仓库 `vendor/`。

### 自己实现的部分

| 环节 | 做法 |
|---|---|
| 检测前处理 | 长边缩到 960、补齐 32 的倍数、ImageNet 均值方差归一化、NCHW |
| 检测后处理 | 概率图二值化(0.3) → 8 邻域连通域 → 框内均值过滤(0.6) → 按 `area×1.5/perimeter` 外扩 → 阅读顺序排序 |
| 识别前处理 | 高固定 48、按纵横比定宽(32–960)、`[-1,1]` 归一化、不足宽补 0 |
| 识别后处理 | CTC 贪心解码（跳 blank、并重复），置信度取保留字符概率均值 |
| 过滤 | 单行置信 < 0.5 丢弃（PaddleOCR 的 `drop_score` 默认值） |

不用 `minAreaRect`：轴对齐外接框在网页/文档的正交排版上足够，省掉凸包旋转卡壳，
也不必引入 opencv。

### 实测

| 场景 | 检测 | 识别 | 合计 | 结果 |
|---|---|---|---|---|
| 1000×560 测试图（8 行） | 80 ms | 161 ms | **242 ms** | 8/8 行，置信 0.97–1.00 |
| 2560×1440 全屏（密集桌面） | 112 ms | 699 ms | **811 ms** | 83 行 |

测试图 `src-tauri/tests/ocr-sample.png` 由 `scripts/make-ocr-sample.ps1` 生成
（GDI+ 渲染、字号 13–34 px、中英混排）。两处错字：`阈`→`阔`、`」`→`]`；
中英混排时词间空格偶发丢失（模型对 space 类的判别本身偏弱，PaddleOCR 同源同病）。

瓶颈是识别：约 **8.4 ms/行**。逐行推理是为了避免把短行 padding 到最长行白白多算，
代价是调度开销；全屏 80+ 行时它才成为主项，单行/局部截图场景可忽略。
下一轮可做小批（同批 padding 到最大宽）再压。

### 内存（用卸载换常驻）

| 阶段 | 主进程常驻 |
|---|---|
| 引擎未加载（托盘态 / 面板回收后） | 11–33 MB |
| 引擎加载后 | **152.7 MB** |
| 面板销毁 → `ocr::unload()` | **32.9 MB** |

面板一销毁就 drop 两个 Session。ORT 的环境对象与 dll 映像留在进程内（约 +10 MB，
`OnceLock` 不释放），仍在 80 MB 红线内。重新加载约 250 ms，被"按热键到窗口出现"的
建窗时间掩掉。

线程数实测（同一张测试图）：

| 线程 | 内存 | 检测+识别 |
|---|---|---|
| 4（默认） | 152.7 MB | 255 ms |
| 2 | 151.4 MB | 328 ms |
| 1 | — | 540 ms |

内存几乎不随线程数变化（主要是模型权重与激活 arena），所以默认 `min(核数, 4)`。

### 自检

```powershell
# 真实屏幕
$env:SHOTGLANCE_OCR_SELFTEST='1'; .\src-tauri\target\release\shotglance.exe

# 固定图片（可复现，脚本可断言行数与文本）
$env:SHOTGLANCE_OCR_IMG='src-tauri/tests/ocr-sample.png'
$env:SHOTGLANCE_OCR_SELFTEST='1'; .\src-tauri\target\release\shotglance.exe
# 结果写到 %TEMP%\shotglance-ocr-selftest.txt
```

| 环境变量 | 作用 |
|---|---|
| `SHOTGLANCE_MODELS` | 覆盖模型目录 |
| `SHOTGLANCE_ORT_DLL` | 覆盖 onnxruntime.dll 路径 |
| `SHOTGLANCE_ORT_THREADS` | 覆盖 intra 线程数 |
| `SHOTGLANCE_OCR_IMG` | 用图片代替截图，做可复现识别 |
| `SHOTGLANCE_HOLD_SECS` | 自检后保持进程存活 N 秒，供外部采样内存 |
| `SHOTGLANCE_UNLOAD_TEST=1` | 保持前先卸载引擎，验证回到基线 |
| `SHOTGLANCE_TOPMOST=1` | 面板置顶（截屏取证时窗口会被其它应用盖住） |
| `SHOTGLANCE_AUTO_OCR=1` | 面板加载完自动跑一次识别，便于无人值守取界面证据 |
| `SHOTGLANCE_DUMP_IMG=<path>` | 采集自检时把那一帧落盘成 JPG |

## 目录

```
src-tauri/src/
  main.rs        入口
  lib.rs         托盘、全局热键、按需建窗与空闲回收、选区形态、命令注册
  capture/       屏幕采集（xcap + WGC）、编码、预热、区域裁剪、无界面自检
  ocr/
    mod.rs       ort 会话管理、模型寻址、加载/卸载、推理编排
    det.rs       DB 检测前处理 + 连通域后处理 + 阅读顺序
    rec.rs       CRNN 前处理 + CTC 解码
  translate/     翻译层（Rust 模块名 mt，避免与 fn translate 同名）
    mod.rs       配置持久化、引擎注册表、对冲竞速、失败熔断、分段
    online.rs    DeepL / OpenAI 兼容 / Google / MyMemory
    offline.rs   Opus-MT ONNX 编解码（自实现 SentencePiece）
  models/        PP-OCRv4 det/rec + 字典（15 MB）
  assets/mt/     tokens.tsv（1.8 MB，翻译词表，随包分发）
  vendor/        onnxruntime.dll（19 MB）
  tests/         ocr-sample.png（OCR 准确率基准图）
src/             前端（原生 TS，无框架）
release/         已构建的安装包（22 MB）
scripts/
  export-snapshot.ps1      白名单打包迁移快照（新增文件记得加进列表）
  restore-nsis-cache.ps1   离线补齐 NSIS 打包工具链
  measure-memory.ps1       窗口生命周期与内存三阶段实测
  make-ocr-sample.ps1      生成 OCR 基准测试图（GDI+ 渲染，必须存成 UTF-8 BOM）
  gen_icon.py              重生成图标（纯 stdlib）
  mt/                      离线翻译模型转换脚本（不参与构建）
  vendor/                  NSIS 安装文件（2.3 MB，随项目走）
```

## 当前状态

- [x] 项目骨架、托盘、按需建窗 + 空闲回收（空闲 11.5 MB / 回收后 32.9 MB，达标）
- [x] 全局热键 Ctrl+Shift+S（注册失败自动降级为托盘触发）
- [x] 截屏采集：多屏枚举 + 整屏采集 + JPEG 预览（预热后 121–130 ms，达标）
- [x] 前端界面、预览区、invoke 通道
- [x] NSIS 安装包可离线构建
- [x] 区域框选（复用面板窗口铺满屏 + 冻结帧裁剪，坐标按渲染矩形反算）
- [x] PP-OCRv4 推理接入（测试图 8/8 行、全屏 83 行；242 ms / 811 ms）
- [x] OCR 引擎按需加载 + 面板销毁即卸载（152.7 MB → 32.9 MB）
- [x] 翻译层：5 引擎可切换 + auto 对冲竞速 + 离线 Opus-MT（详见 HANDOFF §1.2）
- [x] 译文悬浮层（复用面板窗口变形为无边框置顶卡片，贴原文框下方，详见 HANDOFF §1.3）
- [x] OCR 后自动翻译（框选松手即出译文；设置里可关，避免按量计费引擎产生意外费用）
- [ ] 离线解码提速（merged decoder + KV cache，预计快 3–5 倍）
