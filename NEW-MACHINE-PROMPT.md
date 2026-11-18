我要在这台机器上接手 ShotGlance 项目（Rust + Tauri 2 的截图 → OCR → 翻译桌面工具）。请把迁移包还原成可构建的环境，环境验收通过之前不要动功能代码。

## 一、你手上的输入（先确认，别假设）

两个文件在同一个目录里：

| 文件 | 说明 |
|---|---|
| `shotglance-upload-20261113.zip` | **131.25 MB**，SHA256 `7a6dfea2afb31eea4669a2eef0ae6853c9928f0c6390127bd33589028e73cb39` |
| `NEW-MACHINE-PROMPT.md` | 本文件（指令，不用解压） |

外层 zip **可能还没解压**。没解压就由你来解，不要让我手工做。它内含 6 个文件：

- `shotglance-code.zip` — 代码 HEAD 快照，含 `.workbuddy/memory/` 全部记忆
- `shotglance-history.bundle` — 完整 git 历史（5 个提交）
- `ShotGlance_1.0.1_x64-setup.exe` — 成品安装包 21.88 MiB（v1.0.1，仓库根构建区 `build/release/` 的归档副本）
- `cargo-registry.tar`（crate 缓存 + index）、`node_modules.tar`（前端依赖）
- `SHA256SUMS.txt`

**两个目录概念，后面所有命令都依赖这个区分：**

- **素材目录 `M`** = 外层 zip 解压后的目录（放上面 6 个文件）。建议 `<工作区上级>\sg-upload`
- **工作区根 `W`** = `shotglance-code.zip` 解压后的目录（放 `shotglance\`、`.workbuddy\`、`README.md` 等）。**路径随意**，不要求固定，见第四节

开工前先把 `M` 和 `W` 的绝对路径解析出来并告诉我，因为后面的 `tar` / `git fetch` 命令都相对它们。

## 二、执行顺序（每步做完报结果）

1. **校验外层包**：SHA256 必须等于上面那串。不一致就停下，不要继续解压。
2. **解压外层 zip 到 `M`**，然后按 `M\SHA256SUMS.txt` 逐项校验 5 个内部文件。
3. **解压代码到 `W`**：`Expand-Archive M\shotglance-code.zip -DestinationPath W`。
   解压后 `W` 根下应当是平级的：`shotglance\`、`.workbuddy\`、`README.md`、`MIGRATION-2026-11-13.md`、`verify-env.ps1`。
   `shotglance\` 才是工程本体，**不要再套一层目录**；如果发现多套了一层，纠正后再继续。
4. **现在才读文档**（顺序很重要，没解压就没有这些文件）：
   - `W\.workbuddy\memory\MEMORY.md` — 长期记忆：项目定位、不可回退的技术决策、环境基线
   - `W\.workbuddy\memory\2026-11-13.md` — 最近一次施工日志：扁平化、环境恢复、编译器安装、打包验收全过程
   - `W\MIGRATION-2026-11-13.md` — 本次迁移的取舍与还原步骤
   - `W\shotglance\HANDOFF.md` — 环境与恢复细节、踩坑清单
   读完先给我一份 5 行以内的现状摘要，确认你理解了再往下。
5. **还原 git 历史**（在 `W` 下）：
   `git init -q` → `git fetch M\shotglance-history.bundle "refs/heads/*:refs/heads/*"` → `git reset --hard HEAD`
   然后确认 `git log --oneline` 有 5 个提交、`git status` 干净。
6. **还原加速包**：
   `tar -xf M\cargo-registry.tar -C $env:USERPROFILE\.cargo\registry`（目录不存在先建）
   `tar -xf M\node_modules.tar -C W\shotglance\`
7. **查工具链现状**（先查再装，别直接装）：
   - `cargo --version`、`rustc --version`、`node --version`
   - MSVC：`C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`
   - SDK：`C:\Program Files (x86)\Windows Kits\10\Include` 下有哪些版本目录
   缺 Rust → 装 rustup 并指定 `1.99.1`（**msvc** 工具链，默认已静态链 vcruntime，不需要 `+crt-static`）。
   缺 MSVC/SDK → 见第三节。**这台机器如果之前装过 VS，先跑 vswhere 确认，可能省掉 58 分钟。**
8. **环境自检**：跑 `W\verify-env.ps1`，要求除 Python 外全 PASS（Python 3.10/3.11 只影响重新导出 OCR 模型，不影响构建，可忽略）。
9. **构建验收三级**：`cargo check` → `cargo build`（无缓存，全量约 30 分钟）→ `npm run tauri:build`。
   v1.0.0 起构建入口是仓库根 `build.ps1`，产物落在构建区 `build/`（`dist`/`target`/`release`），源码树里不再有 `target` 与 `dist`。
10. **对比基线**：安装包应约 **21.87 MiB**。偏差明显就停下来分析，不要当成"新版本正常"。

## 三、编译器安装（只在第 7 步确认缺失时做）

装之前必须先做两件事，顺序不能反：

```powershell
# 1) 清掉八个代理变量（不清会报 0x80070057）
foreach ($n in @('http_proxy','https_proxy','all_proxy','no_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY')) {
  [Environment]::SetEnvironmentVariable($n, $null, 'Process')
}
# 2) 杀掉残留下载进程（它锁住 Installer 目录，会让安装器报 0x80131500 / exit 5002）
Stop-Process -Name BackgroundDownload -Force -ErrorAction SilentlyContinue
```

然后：

```powershell
.\vs_BuildTools.exe --quiet --wait --norestart `
  --add Microsoft.VisualStudio.Workload.VCTools `
  --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
  --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --includeRecommended
```

退出码 **3010 属成功**（官方建议重启，实测不重启也能编译）。目标基线：MSVC **14.44.35207**、Win SDK **10.0.22621.0 / 10.0.26100.0**、Node **22.22.2**、NSIS **3.13**。

`vs_BuildTools.exe` 本机不一定有，需要联网从 `https://aka.ms/vs/17/release/vs_BuildTools.exe` 取。

## 四、路径可以随便放（这是刻意的）

`src-tauri/target` 这次没有跟着迁移，会从零重建，所以历史上那条"工作区绝对路径必须完全一致才能复用编译缓存"的限制**本次自然失效**。`W` 放 `D:\dev\` 还是用户目录都无所谓，不必为了缓存去固定路径。

## 五、三个必须照做的坑

1. **装编译器前先杀 `BackgroundDownload.exe`**。判断"是句柄锁还是权限问题"的口诀：目录内子项能改名、目录本身不能改 → 句柄被占；两者都不能 → 权限问题。终极定位用 Sysinternals `handle64.exe -nobanner -accepteula -a "<路径>"`。
2. **打包前清代理 + 离线部署 NSIS**。不清代理会报 `CONNECT proxy failed: proxy server responded 502/502`；NSIS 一旦让 Tauri 自己联网下就会 DNS 失败，先跑 `powershell -ExecutionPolicy Bypass -File W\shotglance\scripts\setup-nsis-cache.ps1`（它校验 `scripts/vendor/` 两个文件的 SHA1 与 tauri-bundler 源码常量一致，把 zip 内容铺平到 `%LOCALAPPDATA%\tauri\NSIS` 根，插件放 `Plugins\x86-unicode\additional\`，最后校验 13 个必需文件）。
3. **别对体积做线性外推**。VS 安装末期若"下载速度"飙到远超带宽（如 19 MB/s），那是在本地解包铺文件，不是下载，剩余时间会远小于按体积算出来的值。

## 六、约束

- **环境没验收通过之前不要动功能代码。** 功能顺序：译文悬浮层 → OCR 后自动翻译 → 离线解码提速。
- 某步失败时，先把报错原文和你对根因的判断告我，不要反复重试同一条命令。
- 这个仓库有 git 历史，后续每次改动都要提交。`.gitignore` 用一条 `/build/` 挡住整个构建区，另有 `env-bundle/`、`node_modules/`、快照 zip。
- 旧方案 `env-bundle/` 和 `scripts/restore-env-bundle.ps1` 本次不适用（包里也没有 `env-bundle`），不要拿它恢复环境。
- 如果暂时只想验证工具能不能用、不急着构建：直接双击 `M\ShotGlance_1.0.1_x64-setup.exe` 安装即可，不装编译环境也能跑。
