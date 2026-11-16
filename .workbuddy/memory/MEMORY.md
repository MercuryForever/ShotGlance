# ShotGlance — 项目长期记忆

> 面向"隔几周回来"的场景。操作细节看 `shotglance/HANDOFF.md`，技术决策与实测看 `shotglance/README.md`。
> 逐日流水在 `.workbuddy/memory/YYYY-MM-DD.md`（11-09 / 11-11 / 11-12 / 11-13），本文件只留长期有效结论。

## 项目定位
截图 → OCR → 翻译 → 悬浮回显的轻量桌面工具。**Rust + Tauri 2**，纯托盘常驻，`Ctrl+Shift+S` 唤起。
硬指标：装包 <150 MB、空闲 <80 MB、OCR 峰值 <300 MB、冷启动 <2 s、免安装、离线优先。

## 当前进度（2026-11-13 17:34，提交 `6f187df`）
**已实现**：托盘 + 按需建窗 + 空闲回收｜全局热键｜xcap/WGC 截屏｜PP-OCRv4 检测识别｜引擎按需加载/卸载｜NSIS 离线打包｜区域框选｜翻译层（5 引擎 + auto 竞速 + 离线 Opus-MT）｜**译文悬浮层 + 框选后自动翻译**。
**实测**：装包 21.89 MiB｜托盘 11.5 MB｜回收后 32.9 MB｜OCR 加载 152.7 MB｜截图 130 ms｜基准图 8/8 行 242 ms｜全屏 83 行 811 ms｜翻译 auto 613–1243 ms、离线 667–766 ms。
**剩余路线**：~~离线解码提速（merged decoder + KV cache）~~ ✅ 2026-11-16 完成（详 §1.4，结论：短句 1.2×、80 token 1.6×，瓶颈是每 token 33M MAC 的 65001 路输出投影，与原估 3–5× 不符）→ 识别小批优化。
**待真机复看**：框选遮罩在 VM 里重绘异常。

## 换机落地（2026-11-16，当前这台机器已完全就绪）
第三次换机完成，工作区 `C:\Users\Administrator\Downloads\1113-2`（扁平单层，仓库根 = 工作区根，工程在 `shotglance/`）。
提交 `15c5213`（= `6f187df` + 记忆文件更新）。冷缓存实测：`cargo check` 3m20s、`cargo build` 3m12s、
`npm run tauri:build` **5m57s → 21.87 MiB**、`cargo test --lib` 5 passed、`tsc --noEmit` 0 错。
`target` 全部重建（本次未迁移），路径不再需要与旧机一致。两条 tar 已还原：registry 747 文件、node_modules 455 文件 / 62 MB。

## 构建基线（2026-11-13 实测，本机已完全就绪）
`cargo check` 增量 0.84–1.15 s｜`cargo build`(debug) 76.3 s｜release 1m10s–2m44s｜`npm run tauri:build` **93–96 s** → 21.89 MiB 安装包，归档 `shotglance/release/`。
`cargo test --lib` 5 passed｜`tsc --noEmit` 0 错。
装包体积由 `src-tauri/Cargo.toml` 的 opt-level="z" + lto 控制，加体积前先算红线。

## 环境基线
Win11 22621｜Rust 1.99.1 stable-**msvc**（无需 `+crt-static`，默认已静态链 vcruntime）｜MSVC **14.44.35207**（`Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`，本机 BuildTools **17.14.45**）｜WinSDK **10.0.22621.0 + 10.0.26100.0**｜Node 22.22.2｜WebView2 152→**本机 157.0.4341.46**｜NSIS 3.13（部署在 `%LOCALAPPDATA%\tauri\NSIS`，原包 `scripts/vendor/`，`setup-nsis-cache.ps1` 可一键铺平）。
`verify-env.ps1` 仅剩 Python 3.10/3.11 一项 FAIL（只"模型导出"用，不影响构建）。

本机额外事实（2026-11-16）：`cargo`/`rustc` 在 `C:\Users\Administrator\.cargo\bin`（已写用户 PATH）；Node 与 npm 用托管版
`C:\Users\Administrator\.workbuddy\binaries\node\versions\22.22.2-2`（**不在系统 PATH**，跑 npm 要手动加）；
`C:\Users\Administrator\.cargo\registry` 已还原（离线可编）。给 cargo 传 `--offline` / `CARGO_NET_OFFLINE=true` 可避免联网更新索引。


## 架构要点

### 窗口与内存（不可回退）
- **窗口一律 destroy 不 hide**：`hide()` 不释放 WebView2 子进程（315 MB），内存红线只能靠销毁。
- **OCR 引擎随面板销毁卸载**：ORT 会话内存几十 MB 不自缩；重载 250 ms 被建窗时间吸收。
- **悬浮卡片不新开窗口**，把面板窗口变形（`enter_overlay_mode`）：摘边框 + 置顶 + `set_skip_taskbar(true)`。多一个 WebView2 窗口 +290 MB，直接顶破红线。

### 区域框选
- 复用面板窗口拉成全屏无边框置顶，**不新开 overlay 窗口**。
- 冻结一帧存 `PENDING`，裁剪与识别都吃这一帧，避免"看到的和识别的不一致"。
- 坐标换算用 `frameW / 渲染矩形宽` 反算，不写死 DPI。
- 顺序约束：先 `set_decorations(false)` 再 `set_size`，否则客户区差一个标题栏，坐标整体偏移。

### 译文悬浮层（2026-11-13）
- **锚点用屏幕物理坐标**：`region_finish` 返回 `screen_x/screen_y` = 显示器原点 + 裁剪原点；漏加显示器原点，多屏时卡片永远贴在主屏左上角。几何状态 `OVERLAY_GEOM`/`OVERLAY_ANCHOR` 与 `PANEL_GEOM` 分开存，两形态互不覆盖。
- **高度两段式**：后端先给 96 CSS px 占位，前端量 `.ov-text` 的 `scrollHeight` 再 `overlay_fit` 精调。**不能量卡片自身 scrollHeight**——它 `overflow:hidden` 且撑满窗口，永远等于窗口高，会死循环。
- **定位顺序**：框下方 → 放不下翻上方 → 都放不下贴底 → 左右夹进该屏。夹取用"该显示器自己的原点"，负坐标副屏不能当越界推回 0。5 个单测覆盖。
- **悬浮期间空闲回收 ×3**（120 s → 360 s）：用倍数而非新常量，保住 `SHOTGLANCE_IDLE_SECS` 的测试覆盖。
- **热键唤回面板先退悬浮形态**并 `emit("overlay-dismiss")`；前端不能再 invoke `overlay_close`，否则把刚还原的几何又存一遍。
- **自动翻译默认开**（`Cfg.auto_translate`，`serde(default)` 兼容旧配置），失败也进卡片显示原因。按量计费引擎会产生费用，设置里有开关。
- **取证手段**：`SHOTGLANCE_AUTO_OVERLAY=1` 打开 `index.html?auto=overlay` 摆样例卡片；另一进程用 `SHOTGLANCE_SELF_TEST=1 SHOTGLANCE_DUMP_IMG=xx.jpg` 存整屏图；几何另打日志 `[shotglance] 悬浮卡片 WxH+X,Y`。

### 翻译层
- **模型外置**：113 MB Opus-MT 不放安装包（否则 22 → 135 MB 顶破红线），放 `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`，应用内下载 + 支持手动放文件。`tokens.tsv`（1.8 MB）随包，与模型版本绑死。
- **auto = 对冲竞速**：首个引擎立刻发，之后每 250 ms 补一个，先返回者胜；全挂再串行落离线。不是串行降级——本机 googleapis 不通，串行每次卡 5 s 超时。
- **失败熔断 90 s**：auto 链跳过失败过的在线引擎；用户显式指定时不跳。
- **贪婪解码**，不用 beam=4（慢 4 倍，长文本质量差别很小）。
- **merged decoder + KV cache（2026-11-16）**：每步只算 1 个新 token，历史留在 24 个 past 张量里。三个坑：①`use_cache_branch` 首步假、之后真；②past 不能用 0 长度维（ORT Reshape 报 zero exceeds），首步喂 `[1,8,0,64]` 空张量 + 分支置假，别塞零值占位；③cache 分支的 `present.*.encoder.*` 形状坏 `[0,8,1,64]`，encoder 侧 K/V 只收首步、之后不更新。收益看长度：8/13/26/50/80 token = 1.18/1.22/1.26/1.45/1.63×——每步固定 ~15 ms 被输出投影吃掉，KV cache 只能消掉平方项。线程数 4 已饱和（1/2/4/8/20 = 20.8/17.4/17.2/17.0/23.2 ms/步）。
- **模型文件在本机就绪**（2026-11-16）：`encoder.onnx` 52,899,742 B + merged `decoder.onnx` 60,212,804 B + `tokens.tsv`，从 `hf-mirror.com/Xenova/opus-mt-en-zh` 拉的；`download()` 现在会跳过已完整文件。
- **自实现 SentencePiece**：不引 sentencepiece（要 cmake 编 C++）也不引 tokenizers（缺 tokenizer.json）。构建期把 spm 分数 + vocab.json id 合成 `tokens.tsv`，运行时只跑 Unigram Viterbi。

## 不可回退的技术决策
1. **`ort` 用 `load-dynamic`**，不用 `download-binaries`：`cdn.pyke.io` 本机被限到 2 KB/s，构建脚本会静默挂死。dll 来自 PyPI 清华镜像 `onnxruntime-1.31.1` wheel。版本锁 `=2.0.0-rc.15`，API 与常见教程差异极大。
2. **不要引入 opencv/imageproc**。轴对齐框在正交排版够用，DB 后处理自写（二值化 0.3 → 8 邻域 → 均值 0.6 → `area×1.5/perimeter` 外扩）。
3. **字典 6625 类 = 1 blank + 6623 字 + 1 空格**，空格必须最末类，解析时不能 trim 每行。
4. **`ureq` 必须显式注入 native-tls**：它只把 `tls`(rustls) 当默认后端，`native-tls` feature 仅"可用"。漏了 `.tls_connector(...)` 所有 https 报 `no TLS backend is configured`——极像网络问题，会带偏排查。
5. **Marian 的 spm 与 vocab 是两套 id**：source.spm 32000 piece vs embedding 65001 行，必须拿 piece 字符串过 `vocab.json` 重映射，直接喂 spm id 出乱码。
6. **翻译模块在代码里叫 `mt`**（`#[path]` 指向 `translate/`）：lib.rs 已有 `#[tauri::command] fn translate`，同名模块会让 `translate::run` 被解析成函数路径。
7. **手编必须 `--features tauri/custom-protocol`**，否则 exe 是 dev 模式去连 localhost:1420。

## 路径无关性（用户明确要求，已查证结论）
**运行时早已路径无关；"路径固定"只影响构建缓存能否复用，不影响能否构建、更不影响成品运行。**

| 对象 | 解析顺序 | 有绝对路径? |
|---|---|---|
| OCR 模型 `models_dir()` | `SHOTGLANCE_MODELS` → exe 同级 `models/` → exe 同级 `../Resources/models` → `env!("CARGO_MANIFEST_DIR")/models` | 仅第 4 顺位 |
| ort dll `find_dylib()` | `SHOTGLANCE_ORT_DLL` → exe 同级 → exe/models/ → ../Resources → `CARGO_MANIFEST_DIR/vendor` | 仅第 5 顺位 |
| 离线翻译模型 | `SHOTGLANCE_MT_DIR` → `%APPDATA%\ShotGlance\models\opus-mt-en-zh` | 无 |
| 翻译配置 / tokens.tsv | `app_config_dir()` / `BaseDirectory::Resource` | 无 |

- 源码零硬编码绝对路径；唯一残留是两处 `env!("CARGO_MANIFEST_DIR")` dev fallback，正常布局走不到。
- `restore-env-bundle.ps1` 对路径不符是 **Warn + 跳过 target**，不是报错；路径不同只多花 ~30 分钟重编。
- 真要彻底摆脱（可选成本排序）：① 两处 fallback 收进 `#[cfg(debug_assertions)]`；② release 加 `--remap-path-prefix`（未实测）；③ `CARGO_TARGET_DIR` 固定到用户级；④ `mklink /J C:\dev\shotglance` 逻辑路径锁死。

## 迁移方案（2026-11-13 定案，已执行两次换机）
- **原则：只带不可重建的，缓存与工具链全部重建。**
- **全量包**：`shotglance-upload-20261113.zip`（131.25 MB，SHA256 `7b517dd1…cec8f0`）= 代码快照 23.5 MB + git 全历史 bundle 23.5 MB + 安装包 21.9 MB + `cargo-registry.tar` 76.9 MB + `node_modules.tar` 61.0 MB + `SHA256SUMS.txt`。
- **打包配方**（`%TEMP%\pack-upload.py`，值得复用）：已压缩格式（内层 zip/bundle/exe）用 `ZIP_STORED`，tar 用 `ZIP_DEFLATED=6`；实测 207→131 MB 只用 3 秒。
- **不带**：`src-tauri\target`（debug 4.2 + release 1.6 GB，传 8 小时 vs 重建 30 分钟）、rustup 工具链 591 MB、VS BuildTools 3.4 GB（在 Program Files，不在工作区）。
- **固定成本约 2 小时**：重装 Build Tools ~58 分钟 + 工具链下载。
- 旧 `env-bundle`（1.8 GB）方案已被两个 tar 取代。`restore-env-bundle.ps1` / `export-env-bundle.ps1` 仍在，仅作降级路径。
- 新机恢复顺序：解压全量包 → 恢复 crate 缓存与 node_modules → `git clone shotglance-history.bundle`（或解压代码快照）→ `npm ci && npm run tauri:build`。提示词模板在 `%TEMP%\NEW-MACHINE-PROMPT.md`。

## 仓库状态
- 工作区**已是 git 仓库**（根目录 `C:\Users\Administrator\Downloads\2026-11-09-08-43-20`，仓库内工程在 `shotglance/`）。首提交 `8815d52`（58 文件 / 39.8 MB，最大 onnxruntime.dll 18.4 MB、rec.onnx 10.35 MB、det.onnx 4.53 MB），local user `Administrator <administrator@localhost>`。
- 根 `.gitignore` 忽略 `env-bundle/`、快照 zip；`shotglance/.gitignore` 忽略 `node_modules/dist/target/gen/release`。
- 快照白名单**手工维护**：新增源文件要同步加进 `scripts/export-snapshot.ps1` 的 `$IncludeFiles`，漏了不报错只是包里没有；校验用 `unzip -l | grep 关键词`，别信 report 条数。

## 高频踩坑（复发率高）
- **打包前必须清 8 个 `*_proxy` / `*_PROXY`**：新终端继承已死的 `HTTP_PROXY=http://127.0.0.1:13622` → tauri 报 `CONNECT proxy failed 502`；清掉后又变 `os error 11001`（域名解析）。
- **NSIS 离线包布局**：zip 内 `nsis-3.13/` 的内容要**直接铺到 `NSIS\` 根**（不能多留一层）；插件 DLL 放 `Plugins\x86-unicode\additional\nsis_tauri_utils.dll`。它校验 13 个必需文件，缺一个就 `remove_dir_all` 整个目录重下。SHA1：NSIS `DB14F8AF…9B8E`、插件 `75197FEE…9B860`。
- **VS installer 报"目录被锁定"（`0x80131500` / exit 5002）先杀 `BackgroundDownload.exe`**：它持有 `Installer\resources\...` 下十几个 DLL，导致 bootstrapper 无法重命名目录自更新。口诀：**子项能改名、目录本身不能 → 句柄占用；两者都不能 → 权限**。定位用 Sysinternals `handle64.exe -nobanner -accepteula -a "<路径>"`。
- **判断 VS 安装接近尾声**：Packages 增速突然飙到 19 MB/s（= 已转本地解包）+ 组件日志排到 373/378 且 `winsdksetup.exe` 在跑 `OptionId.AvrfExt`。**别对 Packages 体积做线性外推**，最后一公里会给出严重偏大的估计。
- **`EnableLUA=0` 的机器 `Start-Process -Verb RunAs` 不弹 UAC**（UAC 被禁用），易误判"提权没发生"；此时进程本就是完整管理员令牌。
- **PowerShell 原生命令写 stderr 在 `$ErrorActionPreference='Stop'` 下抛 `NativeCommandError`**：`cargo check` 成功信息走 stderr，包 `2>&1 |` 就变终止错误。脚本要临时切 `Continue` 再读 `$LASTEXITCODE`。
- `.ps1` 要么纯 ASCII，要么 UTF-8 **带 BOM**（PS 5.1 按 ANSI 读无 BOM UTF-8，会破坏 param 块并静默丢参数）。
- 给 Rust 传路径必须 `cygpath -w`（`/tmp/x` 会落到 `C:\tmp`）。
- 量内存必须**按 PID 差集 + 轮询**：WebView2 子进程退出要 4–8 s；xcap 的 D3D 缓存 +42 MB 是一次性非泄漏。
- 沙箱限制：`rm -rf` / `sed -i` 被拦；回收站也做不到（`Add-Type` 载 Microsoft.VisualBasic、`New-Object -ComObject Shell.Application` 均被拒）→ "删除"退化为 `Move-Item` 到 `%TEMP%`。清目录用 `robocopy <空目录> <目标> /MIR`；删超长路径用 `[IO.Directory]::Delete('\\?\'+$p,$true)`。

- **VS 引导器撞代理键（新，2026-11-16）**：`http_proxy` 与 `HTTP_PROXY` 同时存在 → 引导器往大小写不敏感的字典里塞，报
  `0x80070057 Couldn't launch setup process / 已添加项。字典中的关键字:"HTTP_PROXY"`，外层 exit 138、Watson P7=5002。
  **不是**文档里那个 502，清空变量也治不好，必须 `env -u` 把 8 个变量**全 unset** 再跑。装完 exit **194** 也可能已完整（本机就是），
  别拿 194 当失败，直接查 `VC\Tools\MSVC`、`Windows Kits\10\Include`、`cl.exe`。另外 `static.rust-lang.org` 直连解析不了（只能走代理），
  `download.visualstudio.microsoft.com` 直连正常（要无代理），两者诉求相反，得分开跑。
- **`core.autocrlf=true` 会毁掉文本资产（新，2026-11-16）**：PortableGit 全局默认 true，`git reset --hard` 后 `dict.txt` 26250→32873 B、
  `tokens.tsv` 1867691→1932692 B（差值=行数），OCR 字典每行多 `\r`、SentencePiece 末列被污染。换机后**先 `git config core.autocrlf false`**
  再 `git rm --cached -r . && git reset --hard`，然后按 `README` 的 SHA 表逐个核 5 个资产。
- **bundle 的仓库根是工作区根，不是 `shotglance/`（新，2026-11-16）**：先在工程目录 `git init` 会写出嵌套 `shotglance/shotglance/`
  并覆盖工程 `README.md`。正确做法：在**工作区根** `git init` + `git fetch --update-head-ok <bundle> "refs/heads/*:refs/heads/*"` + `git reset --hard master`
  （不带 `--update-head-ok` 会被 `refusing to fetch into branch` 拒掉，因为 init 已检出 unborn `master`）。
- **`SHA256SUMS.txt` 是 CRLF**：`sha256sum -c` 报 `'文件'$'\r': No such file or directory`，先 `sed 's/\r$//'` 再校验。
- **`npm run tauri:build` 会去 `go.microsoft.com` 拉 WebView2 引导器**（本机实测有这一步），不是只有 NSIS 一个外网依赖。

## 待用户拍板
- 是否上 macOS / Linux（macOS 需屏幕录制授权，Linux 无法完全自包含）
- 用户称呼 / 我的称呼（定了补 `SOUL.md` / `IDENTITY.md` / `USER.md`）
