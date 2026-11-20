# 更新日志

本项目的所有重要变更都记录在这里。版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)，
格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## [Unreleased]

（暂无待发布变更）

## [1.1.0] - 2026-11-20

**升级须知**：`Ctrl+Shift+S` 从"打开面板"变成"直接进入框选"。想单纯打开面板请点托盘图标，
或按热键后按 Esc（面板会留在屏幕上）。配置文件向后兼容，不需要手工修改。

### 新增

- **两个可配置的截图热键**：全屏截图（默认 `Ctrl+Shift+A`，截屏 + 识别 + 按开关自动翻译）
  与框选截图（默认 `Ctrl+Shift+S`）。设置在设置面板里录制，改完即时生效。
  注册失败会**回滚到旧热键**并提示原因（典型场景：组合被别的程序占用），
  不会出现"配置里写着新热键、实际一个都没注册"。
- **原文框可编辑**：OCR 结果落进来之后能直接改错字；也可以跳过截图，
  把要翻的文本粘贴/输入进去再点「翻译」。手动文本与 OCR 结果走同一条翻译入口。
- **关闭窗口行为可配置**：驻留托盘（默认）/ 直接退出程序。
- **设置入口重新设计**：从「译文」分区里的小齿轮（`padding: 3px 7px`）提到 header，
  改为带文字按钮。原来的问题不只是小——它只在译文分区出现，用户在截图区根本找不到。
- 新增取证钩子 `SHOTGLANCE_AUTO_CFG=1`（对应 `?auto=cfg`），与已有的
  `?auto=ocr` / `?auto=overlay` 同一套用法，用于无法模拟点击时截取设置界面。

### 变更（会改变已有习惯）

- **`Ctrl+Shift+S` 不再是"打开面板"，而是直接进入框选**。原先按热键只把面板叫出来，
  再点「框选」；现在按下去就铺满屏幕等框选。想单纯打开面板请点托盘图标。
- header 右侧的快捷键提示文案改为按当前配置渲染，不再写死。

### 版本

- 五处版本号统一为 **1.1.0**（`package.json` / `package-lock.json` 两处 /
  `Cargo.toml` / `Cargo.lock` / `tauri.conf.json`），安装包
  `ShotGlance_1.1.0_x64-setup.exe`（**21.92 MiB**）

### 文档

- 「版本约定」补齐 **MAJOR** 规则，判据收紧为「升级后要不要额外做什么」
  （必须改配置 / 必须换流程 / 装不上 三者之一），而不是「感觉变没变」
- 格式对齐 Keep a Changelog：补 `[Unreleased]` 段、版本标题的日期改用 ASCII 连字符、补底部链接定义
- `0.1.0` 段引用的提交 hash 改为作者信息重写后实际可点的 `16d8b8f`（旧 hash 已不在历史链上）

## [1.0.0] - 2026-11-19

首个对外发布版（Windows x64）。截图 → OCR → 翻译 → 悬浮回显的完整链路闭合，
七项硬指标全部达标。安装包 **21.89 MiB**；Rust 单测 **11 passed**、`tsc --noEmit` 0 错。

> NSIS 输出不是确定性的（内嵌时间戳/压缩游程），同一份源码两次打包会差几 KB，
> 所以体积数字只按量级看，别拿字节数当构建校验值。

### 修复

- **单实例保护**（新增 `src-tauri/src/single_instance.rs`，约 200 行含注释）。
  第二个进程在 `tauri::Builder` 之前就被拦下：`CreateMutexW` 拿到 `ERROR_ALREADY_EXISTS`
  即判定"已有一个在跑"，`SetEvent` 通知首实例后 `exit 0`——不建窗、不建托盘、不注册热键。
  首实例的监听线程收到信号，通过 `run_on_main_thread` 调 `open_panel()`，
  自动完成"退出悬浮形态 → 还原几何 → 前置"。

  修复前的实际症状：连点两次图标就有两个托盘图标、两份 WebView2 与 ONNX 会话；
  全局热键只有先到的那个注册成功（第二个在建窗时打印
  `全局热键注册失败: HotKey already registered`），而用户点到的往往是后起的那个，
  按 `Ctrl+Shift+S` 毫无反应，看起来就是"软件坏了"。两个实例还会同时写
  `%APPDATA%\ShotGlance\` 下的配置与模型下载，能写出半截文件。

- **内核对象放在 `Local\` 命名空间**：作用域是当前登录会话。同一台机器两个 Windows 用户、
  或两个 RDP 会话各跑一个实例是合理的，用 `Global\` 会把对方踢掉。
- **首实例在建窗之前就把唤回事件建好**（`claim()` 里，不等到 `setup`）：
  否则用户"启动过程中再双击一次"时第二个进程 `SetEvent` 会落空。
  事件是 auto-reset，所以信号会一直留到有人 `wait`，不会丢。
- **自检/取证进程豁免**：`SHOTGLANCE_SELF_TEST` / `SHOTGLANCE_OCR_SELFTEST` /
  `SHOTGLANCE_MT_SELFTEST` / `SHOTGLANCE_MT_BENCH` / `SHOTGLANCE_AUTO_OCR` /
  `SHOTGLANCE_AUTO_OVERLAY` 不参与单实例判定。不豁免的话，托盘实例开着时跑脚本会被判成
  第二实例、静默退出（退出码 0、无任何输出），排查成本极高。

- **截图不再把应用自己的窗口拍进去**。抓屏抓的是"屏幕上有什么"，而面板就摆在屏幕上：
  全屏截图会整块带上窗口，框选则把它留在底图里，用户框到那块区域时截图里就出现一块
  "上一次的画面"。三条采集路径（`capture_screen` / `ocr_screen` / `region_begin`）统一改为
  `hide_panel` → 等 `hide_settle_ms()`（默认 120 ms，`SHOTGLANCE_HIDE_MS` 可调）→ 抓帧
  → `show_panel`。等待是必需的：`hide()` 只告诉系统该隐藏，真正从画面消失要等 DWM
  下一次合成，抓帧可能拿到更早的那一帧。抓帧失败时同样把窗口还回去。

- **框选结束不再留下一块深色残影**。原顺序是「先让后端缩窗、等 OCR 结果回来才撤
  `body.selecting`」，于是整段裁剪 + 识别（100–700 ms）里窗口只有面板大小，里面却是
  选区态的纯深色空块——既像窗口没消失，又像上一帧的残影。改为「撤选区态 → 等两帧
  （`requestAnimationFrame` 是"这一帧已绘制完成"的确定性信号，不用固定 sleep）→ 才发命令」，
  `cancelRegion` 同步重排。

### 依赖

- 新增直接依赖 `windows-sys 0.59`（`Win32_Foundation` / `Win32_Security` / `Win32_System_Threading`）。
  没有引 `tauri-plugin-single-instance`：它要求联网补 crate，而交付包的离线
  `cargo-registry.tar` 快照里没有；`windows-sys` 本来就是 tauri 的传递依赖（已在 `Cargo.lock`）。

### 实测

| 项 | 结果 |
|---|---|
| 首实例纯托盘（`SHOTGLANCE_NO_PANEL=1`） | 1 进程 / 12,424 K / 无窗口 |
| 再启动一次 | 退出码 **0**、**564 ms** 退出、输出"已有实例在运行，已请求它前置面板" |
| 唤回后 | 仍 **1 进程**（同一 PID）、67,028 K、窗口标题 `ShotGlance` |
| 自检豁免 | 托盘实例在跑时 `SHOTGLANCE_SELF_TEST=1` 正常产出文件，进程数不变 |

### 版本

- 版本号统一为 **1.0.0**（`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`），
  安装包 `ShotGlance_1.0.0_x64-setup.exe`。

### 仓库结构（Git 区 / 构建区）

- **Git 区**（入库）：仓库根的 `.gitignore` / `.gitattributes` / `README.md` / `LICENSE`（MIT）/
  `CHANGELOG.md` / `build.ps1` / `verify-env.ps1`，以及工程目录 `shotglance/`
  （前端 `src/`、`src-tauri/`、`scripts/`、全部构建配置）。
- **构建区**（不入库）：仓库根 `build/`，下辖 `dist`（vite 前端产物）、`target`（cargo 编译输出 +
  `release/bundle/nsis` 安装包）、`release`（归档的安装包）、`logs`（构建转录）。
  根 `.gitignore` 用一条 `/build/` 整体排除。
- 三条路径连线：`vite.config.ts → outDir: "../build/dist"`；
  `src-tauri/tauri.conf.json → frontendDist: "../../build/dist"`；
  `src-tauri/.cargo/config.toml → build.target-dir = "../../build/target"`。
- 构建配置**留在 Git 区是刻意的**：npm / cargo / vite / tauri 都按固定相对路径查找配置，搬走即断。
  「构建脚本」这个角色因此交给根目录 `build.ps1`——它只负责把产物写进 `build/`，自己不存配置。
- 版本号统一为 **1.0.0**（`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`），
  安装包文件名随之变为 `ShotGlance_1.0.0_x64-setup.exe`。
- 新增 `.gitattributes`：把 `*.onnx` / `*.dll` / `dict.txt` / `*.tsv` 钉成 `-text`，
  防止 `core.autocrlf` 再次破坏 OCR 字典与 SentencePiece 词表。
- 新增 `LICENSE`（MIT）。新增根 `build.ps1` 统一构建入口（带 `-Clean` / `-DebugBuild` /
  `-Target Frontend|Backend`，并把安装包归档到 `build/release`）。

### 新增（体验）

- **托盘图标**： `TrayIconBuilder` 显式传入 32×32 PNG（编译期内嵌）。此前没传 `icon()`，
  `tray-icon` 把 `hIcon` 置为 null，Windows 托盘区只占位不显示图形。悬停提示
  `ShotGlance — Ctrl+Shift+S 打开面板`；左键单击/双击打开或前置面板；右键出菜单
  （显示 / 隐藏面板、退出）。
- **启动即显示主窗口**：双击图标直接看到界面，不再只留一个托盘图标让人猜"软件到底启动了没"。
  窗口关闭后照旧销毁并驻留托盘，热键 `Ctrl+Shift+S` 随时唤回。
- **安装器简体中文**：`nsis.languages = ["SimpChinese","English"]` + `displayLanguageSelector`。
  安装前弹语言选择框，卸载程序跟随所选语言；静默安装按系统 UI 语言选。
  实测：选中文后向导标题 `ShotGlance 安装`、正文「欢迎使用 ShotGlance 安装程序」、
  按钮「下一步(N) >」/「取消(C)」。注：选择框自身的标题仍是英文，
  且注册表里若有旧的 `Installer Language` 值会跳过选择框（见 README）。

### 变更（会改变已有习惯）

- **托盘左键由「切换显示/隐藏」改为「打开/前置」**。启动即开窗之后再切换就危险了：
  窗口本来在前台，用户点托盘想把它叫出来，结果被关掉。隐藏改由右键菜单负责。
- **启动窗不做空闲回收**。窗口晾在屏幕上超过 120 秒不再自动消失（否则像崩溃）；
  由热键/托盘打开的面板仍按 120 秒回收，内存红线不受影响。
- 环境变量：`SHOTGLANCE_OPEN_PANEL=1` 已成为默认行为，因此移除；新增
  `SHOTGLANCE_NO_PANEL=1`（回到纯托盘态，量内存基线用）与
  `SHOTGLANCE_RECLAIM_STARTUP=1`（让启动窗也参与回收，`measure-memory.ps1` 阶段 C 用）。

## [0.1.0] - 2026-11-16

**首个可交付版本（Windows x64）。** 截图 → OCR → 翻译 → 悬浮回显的完整链路已经闭合，
七项硬指标全部达标，可以分发。对应提交 `16d8b8f`。

### 硬指标实测

| # | 指标 | 目标 | 实测 | 判定 |
|---|---|---|---|---|
| 1 | 安装包体积 | < 150 MB | **21.88 MiB** | ✅ 约 6.8 倍余量 |
| 2 | 空闲内存 | < 80 MB | 托盘 **11.5 MB** / 面板回收后 **32.9 MB** | ✅ |
| 3 | OCR 峰值内存 | < 300 MB | **152.7 MB**（引擎加载后） | ✅ |
| 4 | 冷启动到首窗 | < 2 s | **87–112 ms** | ✅ 约 20 倍余量 |
| 5 | 截图耗时 | < 300 ms | 稳态 **130 ms**（冷启动首帧 351 ms 属正常） | ✅ 约 2.3 倍余量 |
| 6 | 离线翻译（≤30 输出 token） | < 500 ms | **25–80 ms** | ✅ 约 6 倍余量 |
| 7 | 单文件免安装 | 要求 | NSIS `currentUser` + WebView2 `embedBootstrapper` | ✅ 无需管理员权限 |

测量口径与复现命令见 [`README.md`](shotglance/README.md)；「空闲 < 80 MB」只靠窗口回收能达成，
压模型或调线程池都做不到。

### 新增

**常驻与唤起**

- 纯托盘启动（`app.windows` 为空数组），无残留窗口
- 全局热键 `Ctrl+Shift+S`，注册失败自动降级为托盘左键触发
- 面板按需建窗、用完 **destroy 不 hide**；无活动 120 秒由 watchdog 自动回收
  （悬浮层期间阈值 ×3 放宽到 360 秒，避免「看原文再回来看译文」误回收）

**截屏与框选**

- `xcap` + WGC 采集，多显示器枚举，JPEG q85 预览
- 区域框选：复用面板窗口铺满目标屏、摘边框置顶，底下压「按下那一刻」的冻结帧；
  裁剪与识别都吃这一帧，避免看到的和识别的不一致
- 坐标按 `frameW / 渲染矩形宽` 反算，不写死 DPI；锚点用**屏幕物理坐标**，多屏不串位

**OCR**

- PP-OCRv4 det + rec，ONNX Runtime 1.31.1 走 `load-dynamic`（dll 随包，不依赖系统 PATH）
- 自写前后处理：DB 二值化 → 8 邻域连通域 → 框内均值过滤 → 外扩 → 阅读顺序；
  CRNN 高固定 48 + CTC 贪心解码，单行置信 < 0.5 丢弃
- 引擎按需加载、面板销毁即 `ocr::unload()`：152.7 MB → 32.9 MB
- 基准图 1000×560 共 8 行 **8/8 全部识别**，置信 0.97–1.00，242 ms；
  2560×1440 全屏 83 行 811 ms

**翻译**

- 5 个引擎：DeepL / OpenAI 兼容 / Google / MyMemory / 离线 Opus-MT
- `auto` 采用**对冲竞速**（首个引擎立即发，之后每 250 ms 补一个，先返回者胜），
  不是串行降级——本机 googleapis 不通时串行每次卡 5 秒超时
- 失败熔断 90 秒：auto 链跳过失败过的在线引擎，用户显式指定时不跳
- 离线引擎完全本地：Opus-MT ONNX + 自实现 SentencePiece Unigram Viterbi
  （不引 sentencepiece / tokenizers，免 cmake 与 tokenizer.json）
- 长文本自动分段翻译后拼接

**悬浮回显**

- 译文悬浮卡片**不新开窗口**：把面板窗口变形为无边框置顶卡片
  （`enter_overlay_mode`），多一个 WebView2 窗口会 +290 MB 直接顶破红线
- 定位顺序：框下方 → 放不下翻上方 → 都放不下贴底 → 左右夹进该屏；5 个单测覆盖
- 高度两段式：后端先给 96 CSS px 占位，前端量 `.ov-text` 的 `scrollHeight` 精调
- 框选松手即自动翻译（默认开，设置内可关，避免按量计费引擎产生意外费用）；
  翻译失败也进卡片显示失败原因

**工程与打包**

- NSIS 离线打包链路：`scripts/vendor/` 自带 NSIS 3.13 + 插件，打包全程不联网
- 模型外置：113 MB 翻译模型放 `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`，
  应用内下载或手动放置均可；`tokens.tsv` 随包，与模型版本绑死
- 配置持久化到 `%APPDATA%\com.shotglance.desktop\translate.json`
- 全套验收脚本：内存三阶段、OCR 基准图、离线翻译 bench、无界面自检

### 性能

**离线翻译累计提速 6.1–6.8×**（原始 160/257/517 ms → 现在 25/43/80 ms），分两步：

| 步骤 | 手段 | 单步耗时 |
|---|---|---|
| 基线 | merged decoder + KV cache | 14.84 ms |
| 词表投影 int8 化 | 预转置 int8 权重，`MatMul` 换 `MatMulInteger` | **2.37 ms（6.3×）** |

三句基准译文改造前后**逐字一致**，argmax 序列 20/20 相同。体积代价：模型多存一份
int8 转置权重（+32 MB），运行时反而更省——不再每步分配 133 MB 的 fp32 临时张量。

### 已知限制

见 `README.md` 的[「已知限制」](README.md#已知限制)一节：三条未做的优化（识别小批、
Decoder 布局算子收敛、悬浮卡圆角）与「首次使用需联网下载离线翻译模型 113–146 MB」。

### 平台

- Windows 10/11 x64 only（Windows 11 自带 WebView2；Windows 10 缺 WebView2 时安装包会联网获取）
- 不做 macOS / Linux：macOS 需屏幕录制授权，Linux 无法完全自包含

---

## 版本约定

- **`1.0.x`（PATCH）**：只修 bug、补文档、内部重构；不改任何用户可见行为
- **`1.1.0`（MINOR）**：新增用户可见功能，或撤销 README「已知限制」里任一条技术债；
  旧用法全部继续有效
- **`2.0.0`（MAJOR）**：破坏用户已有的行为或配置 —— 具体指以下三种之一：
  旧配置读不出来（字段语义变更需手工迁移）、旧操作**完全失效且无等价替代**、
  安装标识或升级路径不兼容
- **判据是「用户升级后要不要额外做什么」，不是「感觉变没变」**：新增按钮、多一个设置项、
  把某个入口挪了位置，都属于 PATCH 或 MINOR；只有"必须改配置 / 必须换流程 / 装不上"
  才进 MAJOR。内部重构再大，只要外部行为不变就进 PATCH 或 MINOR
- 打包前必须复核 `README.md` 的发布检查清单与七项硬指标；指标退步**不阻止发版**，
  但必须在 CHANGELOG 里写明退步项与原因，不能当作没发生

[Unreleased]: https://github.com/MercuryForever/ShotGlance/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/MercuryForever/ShotGlance/releases/tag/v1.1.0
[1.0.0]: https://github.com/MercuryForever/ShotGlance/releases/tag/v1.0.0
[0.1.0]: https://github.com/MercuryForever/ShotGlance/releases/tag/v0.1.0
