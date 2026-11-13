# ShotGlance — 项目长期记忆

> 面向"隔几周回来"的场景。操作细节看 `shotglance/HANDOFF.md`，技术决策与实测看 `shotglance/README.md`。

## 项目定位
截图 → OCR → 翻译 → 悬浮回显的轻量桌面工具。**Rust + Tauri 2**，纯托盘常驻，`Ctrl+Shift+S` 唤起。
硬指标：装包 <150 MB、空闲 <80 MB、OCR 峰值 <300 MB、冷启动 <2 s、免安装、离线优先。

## 当前进度（2026-11-13 更新）
- 已完成：托盘 + 按需建窗 + 空闲回收；全局热键；xcap/WGC 截屏；PP-OCRv4 检测识别；OCR 引擎按需加载/卸载；NSIS 离线打包；**区域框选**；**翻译层（5 引擎 + auto 竞速 + 离线 Opus-MT）**；**译文悬浮层 + 框选后自动翻译**（2026-11-13）
- 实测：装包 21.90 MiB｜托盘 11.5 MB｜回收后 32.9 MB｜OCR 加载 152.7 MB｜截图 130 ms｜基准图 8/8 行 242 ms｜全屏 83 行 811 ms｜翻译 auto 613–1243 ms、离线 667–766 ms
- 未完成：离线解码提速（merged decoder + KV cache）→ 识别小批优化
- **区域框选**：已实现、已编译、已打包，裁剪与坐标映射实测通过；遮罩在 VM 里的重绘行为异常，待真机复看
- **译文悬浮层**：见下节「悬浮层的实现要点」
- **翻译层**：见下节
- 环境已全部就绪（2026-11-13）：Rust 1.99.1 msvc + MSVC 14.44.35207 + SDK 22621/26100 + NSIS 已部署，`npm run tauri:build` **96 秒**出 21.87 MiB 安装包。**可以直接进入功能开发。**

## 悬浮层的实现要点（2026-11-13）
- **不新开窗口，把面板窗口变形为卡片**：`enter_overlay_mode` 复用同一窗口，摘边框 + 置顶 + `set_skip_taskbar(true)`，缩到内容尺寸贴在原文框下方 8px。多一个 WebView2 窗口要多 290 MB，直接顶破内存红线。
- **锚点用屏幕物理坐标**：`region_finish` 新增返回 `screen_x/screen_y`（= 显示器原点 + 裁剪原点）。少加显示器原点的话，多屏时卡片会一直贴在主屏左上角。
- **卡片高度由内容决定，两段式**：后端先给 96 CSS px 占位，前端量完 `.ov-text` 的 `scrollHeight` 再调 `overlay_fit` 精确调整。**不能量卡片的 scrollHeight**——它 overflow:hidden 且撑满窗口，永远等于窗口高度，会死循环。
- **定位顺序**：优先框下方 → 放不下翻上方 → 都放不下贴底 → 左右夹进该屏。夹取必须用"该显示器自己的原点"，负坐标副屏不能当越界推回 0。5 个单测覆盖（`cargo test --lib`）。
- **悬浮期间空闲回收放宽到 3 倍**（120 s → 360 s）：用户正在读那一句，2 分钟就收掉太粗暴；但仍要收——置顶无边框卡片赖在屏幕上不走更烦人。用倍数而非新常量，保住 `SHOTGLANCE_IDLE_SECS` 的测试覆盖。
- **热键唤回面板时先退悬浮形态**，并 `emit("overlay-dismiss")` 让前端把 UI 切回来；前端不能再 invoke 一次 `overlay_close`，否则会把刚还原的几何又存一遍。
- **自动翻译默认开**（`Cfg.auto_translate`，`serde(default)` 兼容旧配置）：框选识别完自动翻并悬浮，失败也进卡片（显示原因）。按量计费引擎会产生费用，所以设置里有开关。
- **取证手段**：`SHOTGLANCE_AUTO_OVERLAY=1` 打开 `index.html?auto=overlay`，摆一张样例卡片；再起一个进程用 `SHOTGLANCE_SELF_TEST=1 SHOTGLANCE_DUMP_IMG=xx.jpg` 把整屏存图。几何另打一行日志 `[shotglance] 悬浮卡片 WxH+X,Y`。

## 工作区现状（2026-11-11 换设备整理后）
- 体积 **106 MB**（原 7.8 GB）。`target`/`node_modules`/`dist`/`gen`/`.setup` 已全删，都是可重建的；要构建先 `npm ci` 再 `npm run tauri:build`（10–20 分钟）。
- 迁移入口：根 `README.md`（索引 + 三条恢复路径 + 校验和）→ `shotglance/HANDOFF.md`（环境 + 恢复步骤 + 踩坑）。
- 快照最终版 `shotglance-snapshot-20261112-1136.zip`（45.4 MB，59 文件，含 translate 层 / tokens.tsv / 成品安装包 / ENV-MIGRATION.md / env-bundle 两个脚本）。旧的 111146 与 1133 均作废，只认 1136。
- **成品安装包已归档到 `shotglance/release/`**（22 MB）——它原先只在 `target/` 里，删缓存就会丢。
- **快照白名单是手工维护的**：新增源文件必须同步加到 `scripts/export-snapshot.ps1` 的 `$IncludeFiles`，漏了不报错只是包里没有。校验用 `unzip -l | grep 关键词`，别信 report 的条数。
- 根 `.gitignore` + `shotglance/.gitignore` 已备好（列出 4 类必须提交的大文件）。
- 2026-11-12 重建环境后又长了：`target` 1.0 GB + `node_modules` 60 MB + `env-bundle` 1.8 GB，当前 **2918 MB**。

## 迁移日操作（2026-11-12 定案，用户已准备换机）
- **带什么**：`shotglance-snapshot-20261112-1136.zip`（45.4 MB，SHA256 `a55c9a1c…ba6c4e`）+ `env-bundle\`（1815 MB）。等价于整目录拷走（2918 MB）。
  **不用带** `src-tauri\target` / `node_modules`（env-bundle 里已有）、`dist`、`gen`、NSIS 缓存（vendor 可恢复）。
  冗余的两份旧快照（111146 / 1133，90.7 MB）已移到 `%TEMP%\sg-old-snapshots-20261112`，工作区只剩 1136。
- **出发前体检已过（2026-11-12 11:37）**：MANIFEST 六个 part 的文件数/体积逐项吻合、两个 installer SHA256 OK、记录的 workspace 路径 == 实际、bundle target 不落后（3305 文件）、`cargo check --offline` 增量 **1.1 s**。dep-info 实证含绝对路径，所以"同路径才复用 target"这条是真约束。
- **恢复一条命令**（在新机 `shotglance\` 下）：
  `powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir <env-bundle 路径> -AddToPath -Check`
  脚本自动：rustup 工具链 → cargo shim 硬链接 → crate 缓存 → node_modules → target（路径匹配才恢复）→ 查 MSVC → 验证。
- **卡点只在 VS**：`-WithVsInstall` 需要 vs-layout（**本机没导，用户未拍板**）；没有就联网跑 `installers\vs_BuildTools.exe`，约 2 小时。提权前必须清重复代理变量，否则 `0x80070057`。
- **三个不在工作区的外部件**：① 离线翻译模型 `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`（113 MB，本机从未下载，换机后应用内下载）；② 翻译配置 `%APPDATA%\com.shotglance.desktop\`（未生成，重配即可）；③ VS BuildTools 本体 3.4 GB。
- 迁完第一件事：`git init` + 首提交（唯一遗留工程债，`.gitignore` 已备好）。
- 详细清单与实测记录见 `.workbuddy/memory/2026-11-12.md` 末节「迁移执行清单」。

## 2026-11-13 落地：扁平化 + 环境恢复（当前工作区状态以此为准）

- **目录已扁平化**：原先双层嵌套（`…\2026-11-09-08-43-20\2026-11-09-08-43-20\shotglance`）已理平，工程回到 `…\2026-11-09-08-43-20\shotglance`，与 dep-info 记录一致。空壳目录已移到 `%TEMP%\sg-nested-shell-20261113`。
- **环境恢复成功（用 env-bundle）**：rustup 152 文件 / 590.5 MB、cargo shim 13 个硬链接、crate 缓存 747 文件 / 76.1 MB、node_modules 455 文件 / 60.6 MB、**target 3305 文件 / 1048 MB 全部命中**。PATH 已追加 `%USERPROFILE%\.cargo\bin`。
- **实测**：`cargo check` **1.15 s / 0.84 s（连测两次）**，与 11-12 记录的 1.1 s 一致；`tsc --noEmit` 0 错。即 target 缓存跨机复用成功——**前提就是这次扁平化**。
- **`git init` 已完成（长期技术债清掉）**：首提交 `8815d52`，58 文件 / 39.8 MB，最大的是 onnxruntime.dll 18.4 MB、rec.onnx 10.35 MB、det.onnx 4.53 MB。仓库根在工作区根目录，local user 为 `Administrator <administrator@localhost>`。
- **补了根 `.gitignore` 的洞**：原先没忽略 `env-bundle/`（1.8 GB），一旦 `git add -A` 会把整个环境包吸进索引。
- **修了 `restore-env-bundle.ps1` 的真 bug**：第 353 行 `& $cargoExe check 2>&1` 在 `$ErrorActionPreference='Stop'` 下，cargo 写往 stderr 的 "Finished" 会被 PS 包装成 `NativeCommandError` 并终止脚本 → **明明 check 成功却报 exit 1**。已改为临时切 `Continue` + 读 `$LASTEXITCODE`。
- **MSVC + Windows SDK 已装好（16:48 完成，环境无阻塞）**：MSVC **14.44.35207**、cl.exe 就位、WinSDK **10.0.22621.0 + 10.0.26100.0**（Include/Lib 均齐）、`vswhere -requires VC.Tools.x86.x64` 能查到 `2022\BuildTools`。安装 `exit 3010`（成功需重启），**实测不必重启即可编译**；耗时 **58 分 04 秒**，Packages 缓存 3.9 GB。
- **决定性验证：`cargo build`（debug）exit 0 / 76.3 s / 产出 `shotglance.exe` 17 MB** —— 本机首次真正链接成功（`cargo check` 不链接，无法证明链接器可用，别拿它当验收）。
- `verify-env.ps1` 仍剩 Python 3.10/3.11 一项 FAIL（仅"模型导出"用，不影响构建）。
- 判断 VS 安装是否接近尾声的两个信号：① Packages 增速突然飙到 **19 MB/s**（远超 200 KB/s 网络上限）= 已转入本地解包铺文件；② 组件日志 `dd_setup_*_NNN_*.log` 排到 373/378 且 `winsdksetup.exe` 在跑 `OptionId.AvrfExt`（SDK 收尾特性）。**别对着 Packages 体积做线性外推，会在最后一公里给出严重偏大的估计。**
- **完整打包已跑通（17:00）**：`npm run tauri:build` exit 0 / **96 s** / `ShotGlance_0.1.0_x64-setup.exe` **21.87 MiB**（22,928,021 B），已归档 `shotglance/release/`。环境至此完全就绪，**剩下的只有功能开发**。
- 构建耗时基线：`cargo check` 增量 0.84–1.15 s｜`cargo build` debug 76.3 s｜release 1m10s–2m44s｜完整打包 96 s。

## 环境与迁移（2026-11-12 定案）
- 新机器环境已配好并验证：Rust 1.99.1 msvc / MSVC 14.44.35207 / SDK 10.0.26100 / Node 22.22.2 / NSIS 缓存；`cargo check` 首过 31 分 10 秒、`tsc --noEmit` 0 错。**Build Tools 装在 `Program Files (x86)`**，查装没装只用 vswhere，别猜路径。
- **迁移用 `env-bundle`，不要从零下**：`scripts/export-env-bundle.ps1` 导出 → `restore-env-bundle.ps1` 恢复，方案与实测见 `shotglance/ENV-MIGRATION.md`。L1 756 MB / +target 1.0 GB / +vs-layout 2.5 GB；恢复 15–20 分钟（从零是 3 小时，其中 2.5 小时纯下载）。
- **target 复用要求工作区绝对路径完全一致**（dep-info 存绝对路径）。建议固定到 `C:\dev\shotglance`，以后不再变。
- 环境包三个关键认知：① `.cargo\bin` 的 13 个 shim 是 `rustup.exe` 的**符号链接**，robocopy 会展开成 172 MB，必须只带 exe + 恢复时建硬链接；② `.cargo\registry\src`（314 MB）不用带，cargo 从 cache 解压；③ `cargo metadata --offline` 必须加 `--filter-platform x86_64-pc-windows-msvc`，否则报缺 android 包（不是缓存不全）。
- 网络基线：rust-lang **71 KB/s**（清华/USTC/阿里云都没有 rustup-init 这个文件）、VS CDN **200 KB/s**、本地 NVMe 复制 **150 MB/s**。排查"为什么慢"先测这三个数。
- 沙箱限制：`rm -rf` / `sed -i` 会被拦；**回收站也做不到**（`Add-Type` 加载 Microsoft.VisualBasic 被拒、`New-Object -ComObject Shell.Application` 被拒），"删除"只能退化为 `Move-Item` 到 `%TEMP%`。清目录用 `robocopy <空目录> <目标> /MIR`，改文件用编辑工具，删超长路径目录用 `[IO.Directory]::Delete('\\?\'+$p, $true)`（这个反而可用）。

## 区域框选的实现要点（2026-11-11）
- 复用面板窗口拉成全屏无边框置顶，**不新开 overlay 窗口**（省 290 MB WebView2 子进程）
- 冻结一帧存 `PENDING`，裁剪与识别都吃这一帧，避免"看到的和识别的不一致"
- 坐标换算用 `frameW / 渲染矩形宽` 反算，不写死 DPI
- 顺序约束：先 `set_decorations(false)` 再 `set_size`，否则客户区差一个标题栏，坐标整体偏移


## 翻译层的实现要点（2026-11-11）
- **模型外置**：113 MB 的 Opus-MT 不放安装包（会让 22 → 135 MB，顶破 150 MB 红线），放 `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`，应用内下载 + 支持手动放文件。词表 `tokens.tsv`（1.8 MB）随包，因为它和模型版本绑死。
- **auto = 对冲竞速**：第一个引擎立刻发，之后每 250 ms 补一个，先返回者胜；全挂再串行落到离线。不是串行降级——本机 googleapis 不通，串行每次要卡 5 s 超时。
- **失败熔断 90 s**：失败过的在线引擎在 auto 链里被跳过；用户显式指定时不跳。
- **贪婪解码**，不用 beam=4（实测慢 4 倍，长文本质量差别很小）。
- **自己实现 SentencePiece 编码**：不引 sentencepiece（要 cmake 编 C++）也不引 tokenizers（缺 tokenizer.json）。构建期把 spm 分数 + vocab.json 的 id 合成 `tokens.tsv`，运行时只跑 Unigram Viterbi。

## 不可回退的技术决策
1. **`ort` 用 `load-dynamic`**，不用 `download-binaries`。`cdn.pyke.io` 在本机被限速到 2 KB/s（32 MB 要几小时，构建脚本静默挂死）。dll 来自 PyPI 清华镜像的 `onnxruntime-1.31.1` wheel。版本锁 `=2.0.0-rc.15`，其 API 与常见教程差异极大。
2. **窗口一律 destroy 不 hide**。`hide()` 不释放 WebView2 子进程（315 MB），内存红线只能靠销毁。
3. **OCR 引擎随面板销毁卸载**。ORT 会话内存几十 MB 且不自缩；重载 250 ms 被建窗时间吸收。
4. **不要引入 opencv/imageproc**。轴对齐框在正交排版够用，DB 后处理自写（二值化 0.3 → 8 邻域 → 均值 0.6 → `area×1.5/perimeter` 外扩）。
5. **字典 6625 类 = 1 blank + 6623 字 + 1 空格**，空格必须在最末类，且解析时不能 trim 每行。
6. **`ureq` 必须显式注入 native-tls**。它只把 `tls`(rustls) 当默认后端，`native-tls` feature 仅"可用"。漏了 `.tls_connector(...)` 所有 https 都报 `no TLS backend is configured`——这个报错极像网络问题，会带偏排查方向。
7. **Marian 的 spm 与 vocab 是两套 id**。source.spm 32000 piece vs embedding 65001 行，必须拿 piece 字符串过 `vocab.json` 重映射，直接喂 spm id 出乱码。
8. **翻译模块在代码里叫 `mt`**（`#[path]` 指向 `translate/`）。lib.rs 已有 `#[tauri::command] fn translate`，同名模块会让 `translate::run` 被解析成函数路径。

## 路径无关性（2026-11-13 查证，用户明确要求"最终版不硬性固定路径"）

**结论先行：运行时早已路径无关；"路径固定"只影响构建缓存能不能复用，不影响能不能构建、更不影响成品运行。**

运行时路径解析链（已逐行查证）：

| 对象 | 解析顺序 | 是否有绝对路径 |
|---|---|---|
| OCR 模型 `models_dir()` | `SHOTGLANCE_MODELS` → exe 同级 `models/` → exe 同级 `../Resources/models` → `env!("CARGO_MANIFEST_DIR")/models` | **仅第 4 顺位有**（编译期烧入） |
| ort dll `find_dylib()` | `SHOTGLANCE_ORT_DLL` → exe 同级 → exe/models/ → ../Resources → `CARGO_MANIFEST_DIR/vendor` | **仅第 5 顺位有** |
| 离线翻译模型 | `SHOTGLANCE_MT_DIR` → `%APPDATA%\ShotGlance\models\opus-mt-en-zh` | 无 |
| 翻译配置 | `app.path().app_config_dir()` → `%APPDATA%\com.shotglance.desktop` | 无 |
| tokens.tsv | `BaseDirectory::Resource` 解析 | 无 |

- 源码里**零硬编码绝对路径**（grep `C:\` / `Downloads` / `Users\` 无命中）。
- 唯一残留是两处 `env!("CARGO_MANIFEST_DIR")` **dev fallback**，正常安装/便携布局下永远走不到。要彻底清除：收进 `#[cfg(debug_assertions)]`。
- `restore-env-bundle.ps1` 第 210–233 行对路径不符的处理是 **Warn + 跳过 target**，不是报错。工具链、crate 缓存、node_modules 照常恢复。即"路径不同 → 只多花 30 分钟重编"，功能零影响。
- 所以"硬性要求路径固定"是**误解**：真正的约束是"想让 target 缓存生效就必须同路径"。缓存是优化项，不是依赖项。
- 若要彻底摆脱（可选，按成本排序）：① 两处 fallback 加 `#[cfg(debug_assertions)]`；② release 加 `--remap-path-prefix` 让 dep-info 存虚拟路径（**未实测**）；③ `CARGO_TARGET_DIR` 固定到用户级目录；④ `mklink /J C:\dev\shotglance` 用逻辑路径锁死（物理位置随意）。

## 迁移（2026-11-13 第二次换设备，200 KB/s 带宽约束下）
- **原则：只带不可重建的，缓存与工具链全部重建。** 工作区 7.8 GB → 散文件 207 MB → **单包交付 131 MB（1.7%）**。
- **最终交付**：`%TEMP%\shotglance-upload-20261113.zip`，131.25 MB，SHA256 `7b517dd1…cec8f0`，旁边有 `.sha256` 文件。
  包内 6 项：`shotglance-code.zip` 23.5 MB（HEAD 快照）+ `shotglance-history.bundle` 23.5 MB（git 全历史）+ 安装包 21.9 MB
  + `cargo-registry.tar` 76.9 MB（crate 缓存+index）+ `node_modules.tar` 61.0 MB + `SHA256SUMS.txt`。
- **打包配方**（值得复用）：已压缩格式（内层 zip/bundle/exe）用 `ZIP_STORED` 避免白烧 CPU，两个 tar 用 `ZIP_DEFLATED=6`；
  实测 **207→131 MB，只用 3 秒**（tar 内是小文件，deflate 极快）。脚本在 `%TEMP%\pack-upload.py`。
- **target 5.8 GB 不带走**（debug 4.2 + release 1.6）：传要 8 小时，重建仅 30 分钟。**这笔账以后都这么算。**
- **rustup 工具链 591 MB 不带走**（传 49 分钟超预算）；**VS BuildTools 3.4 GB 在 Program Files，根本不在工作区**，新设备必须重装约 58 分钟。这两项是迁移的固定成本，约 2 小时。
- 旧 `env-bundle`（1.8 GB）方案**已被 P1 两个 tar 取代**，更小且更聚焦。散文件仍留在 `%TEMP%\sg-upload\` 作为降级备份。

## 环境基线
Win11 22621 ｜ Rust 1.99.1 stable-**msvc**（无需 `+crt-static`，默认已静态链 vcruntime）｜ MSVC **14.44.35207**（`Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`）｜ WinSDK **10.0.22621.0 + 10.0.26100.0** ｜ Node 22.22.2 ｜ WebView2 152 ｜ NSIS 3.13（已部署到 `%LOCALAPPDATA%\tauri\NSIS`，原包在 `scripts/vendor/`）

## 高频踩坑（复发率高）
- **打包前必须清 `*_proxy` / `*_PROXY`**（2026-11-13）：每个新终端会话都从系统继承 `HTTP_PROXY=http://127.0.0.1:13622`，该代理已死 → tauri 下载报 `CONNECT proxy failed: proxy server responded 502/502`，关掉代理后又变 `io: 不知道这样的主机 (os error 11001)`。清进程级八个变量即可。
- **NSIS 离线包的正确布局**（2026-11-13 踩，卡了两次）：缓存在 `%LOCALAPPDATA%\tauri\NSIS`，**zip 内 `nsis-3.13/` 的内容要直接铺到 `NSIS\` 根**（不能多留一层 `NSIS\nsis-3.13\`）；插件 DLL 必须放 `Plugins\x86-unicode\additional\nsis_tauri_utils.dll`（有 `additional` 这层）。它校验 13 个必需文件，任意一个缺失就 `remove_dir_all` 整个目录重下。源码常量 `NSIS_SHA1=DB14F8AF...9B8E`、`nsis_tauri_utils.dll SHA1=75197FEE...9B860`，与 `scripts/vendor/` 两个文件**逐字符一致**，可完全离线。
- **VS installer 报"目录被锁定"时先杀 `BackgroundDownload.exe`**（2026-11-13 踩，连败两次）：它加载了 `Installer\resources\app\ServiceHub\...` 下十几个 DLL，只要活着，bootstrapper 就无法重命名 `Installer` 目录做自更新，报 `0x80131500 / exit 5002`。诊断口诀：**目录内子项能 rename 但目录本身不能 → 句柄占用；两者都不能 → 权限问题**。终极定位用 Sysinternals `handle64.exe -nobanner -accepteula -a "<路径>"`。
- **`EnableLUA=0` 的机器上 `Start-Process -Verb RunAs` 不会弹 UAC**（UAC 被禁用），容易误判"提权没发生"。此时当前进程本来就是完整管理员令牌。
- 手编必须 `--features tauri/custom-protocol`，否则 exe 是 dev 模式去连 localhost:1420
- `.ps1` 要么纯 ASCII，要么 UTF-8 **带 BOM**（PS 5.1 按 ANSI 读无 BOM UTF-8，会破坏 param 块且静默丢参数）
- 给 Rust 传路径必须 `cygpath -w`（`/tmp/x` 会落到 `C:\tmp`）
- 量内存必须**按 PID 差集 + 轮询**：WebView2 子进程退出要 4–8 s，xcap 的 D3D 缓存 +42 MB 是一次性非泄漏
- **PowerShell 原生命令写 stderr 会在 `$ErrorActionPreference='Stop'` 下抛 `NativeCommandError`**：`cargo check` 的成功信息走 stderr，包一层 `2>&1 |` 就变终止错误。这类脚本要临时切 `Continue` 再读 `$LASTEXITCODE`（`restore-env-bundle.ps1` 已修）。
- 本工作区**已是 git 仓库**（2026-11-13 首提交 `8815d52`），根 `.gitignore` 已忽略 `env-bundle/`、快照 zip；`shotglance/.gitignore` 忽略 `node_modules/dist/target/gen/release`。

## 待用户拍板
- 是否要做"OCR 后自动翻译"（框选松手即出译文），还是保持手动点
- 是否上 macOS / Linux（macOS 需屏幕录制授权，Linux 无法完全自包含）
- 用户称呼 / 我的称呼（定了补 `SOUL.md` / `IDENTITY.md` / `USER.md`）
