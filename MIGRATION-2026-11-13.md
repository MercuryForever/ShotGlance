# 迁移清单（2026-11-13 17:0x）

> 时间窗：17:40 前上传完。带宽 200 KB/s = **12 MB/分钟**，41 分钟最多约 **490 MB**（理论值，网盘实际会更低）。
> 因此只带走「不可重建」的东西，其余到新设备重建。

## 一句话结论

工作区 **7.8 GB**，本次只带走 **约 210 MB（2.7%）**。代码、历史、资产、记忆全在；
编译缓存和工具链**全部重建**——因为它们的传输成本远高于重建成本（见 §3）。

## 一、包内文件清单（按重要性分级）

> **打包形态**：最终交付为**单个** `shotglance-upload-20261113.zip`（约 165 MB，200KB/s 下约 14 分钟），
> 内含下表 5 个文件 + `SHA256SUMS.txt`。内层两个 `.tar` 在外层 zip 中用 deflate 压缩（约省 40 MB），
> 已压缩格式（zip / bundle / exe）直接 stored，不再重复压缩。外层包校验值见 `shotglance-upload-20261113.zip.sha256`。
>
> 若单包上传中途失败，可退回分批：原始散文件仍保留在 `%TEMP%\sg-upload\`，按 P0 → P1 顺序重传。

| 级别 | 文件 | 体积 | 作用 |
|---|---|---|---|
| **P0** | `shotglance-code.zip` | 24 MB | HEAD 快照，解压即用 |
| **P0** | `shotglance-history.bundle` | 24 MB | 完整 git 历史（4 个提交） |
| **P0** | `ShotGlance_0.1.0_x64-setup.exe` | 22 MB | 成品安装包，新设备可先验证 |
| **P1** | `cargo-registry.tar` | 77 MB | crate 缓存 + 索引，离线编译必需 |
| **P1** | `node_modules.tar` | 61 MB | 前端依赖，省一次 `npm ci` |
| | **散文件合计** | **207 MB** | 打成单包后约 **165 MB** |

### 还原命令（新设备）

```powershell
# 1. 解压代码
Expand-Archive shotglance-code.zip -DestinationPath shotglance-workspace

# 2. 还原 git 历史（可选，但强烈建议）
cd shotglance-workspace
git init -q
git fetch ..\shotglance-history.bundle "refs/heads/*:refs/heads/*"
git reset --hard HEAD

# 3. 还原加速包（P1）
tar -xf ..\cargo-registry.tar -C $env:USERPROFILE\.cargo\registry
tar -xf ..\node_modules.tar -C .\shotglance\
```

## 二、没有带走的东西（重要，别以为丢了）

| 项 | 体积 | 为什么不带 | 新设备怎么补 |
|---|---|---|---|
| `src-tauri/target` | **5.8 GB** | 传输要 8 小时；重建只要约 30 分钟 | `cargo build` 自动生成 |
| `.rustup/toolchains` | 591 MB | 传输 49 分钟，已超预算 | 跑 `rustup-init.exe`（见 §4） |
| `env-bundle/` | 1.8 GB | 旧方案，已被 P1 的两个 tar 取代且更小 | 不再需要 |
| VS BuildTools | 3.4 GB | **在 `Program Files`，根本不在工作区** | 必须重装，约 58 分钟（见 §4） |
| Windows SDK | ~2.3 GB | 同上 | 随 BuildTools 安装 |
| `dist/` | 32 KB | 可由 `vite build` 重建 | 打包时自动生成 |

**target 这笔账要算清**：它现在只值「省 30 分钟编译」，却占 5.8 GB。
200 KB/s 下传它要 8 小时——**任何情况下都不该跟着走**。实测增量编译只要 76 秒（debug）/ 96 秒（全打包），
热缓存带来的提速已经不如从前明显了。

## 三、新设备恢复的最低成本路径

已知这次必须付的两块硬成本：

| 步骤 | 耗时 | 说明 |
|---|---|---|
| 装 VS BuildTools（VCTools + VC.Tools.x86.x64 + Win11SDK） | **约 58 分钟** | 大头，无法绕过。见 §4 的坑 |
| 装 rustup + 工具链 | 20–40 分钟 | 视网络而定 |
| 解压 P0 + P1 | 3 分钟 | |
| `cargo build`（无 target，全量） | 约 30 分钟 | |
| **合计** | **约 2 小时** | 有 P1 缓存能省掉 crate 下载的时间 |

想要更快，唯一办法是**现在就导出 vs-layout 离线包（2.5 GB，约 3.5 小时传输）**——
在 41 分钟窗口内不可能，所以不列入本次。

## 四、新设备上的两个已知坑（务必按这个来）

### 1. 装编译器前先杀残留进程

VS 安装器报「目录被锁定」时，先关掉 `BackgroundDownload.exe`：

```powershell
Stop-Process -Name BackgroundDownload -Force -ErrorAction SilentlyContinue
```

它加载了 `Installer\resources\app\ServiceHub\...` 下十几个 DLL，只要活着，
bootstrapper 就无法重命名 `Installer` 目录做自更新，报 `0x80131500 / exit 5002`。
定位工具：Sysinternals `handle64.exe -nobanner -accepteula -a "<路径>"`。

安装命令（装完退出码 3010 属正常，不必重启即可编译）：

```powershell
foreach ($n in @('http_proxy','https_proxy','all_proxy','no_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY')) {
  [Environment]::SetEnvironmentVariable($n, $null, 'Process')
}
.\vs_BuildTools.exe --quiet --wait --norestart `
  --add Microsoft.VisualStudio.Workload.VCTools `
  --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
  --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --includeRecommended
```

### 2. 打包前必须清代理 + 部署 NSIS

代理不清会报 `CONNECT proxy failed: proxy server responded 502/502`。
NSIS 不要让它联网下（会 DNS 失败），在项目内一条命令离线部署：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-nsis-cache.ps1
```

脚本会校验 `scripts/vendor/` 里两个文件的 SHA1 与 tauri-bundler 源码常量一致，
并按 Tauri 要求把 zip 内容**铺平到 `%LOCALAPPDATA%\tauri\NSIS` 根**、插件放到
`Plugins\x86-unicode\additional\`，最后校验 13 个必需文件。

## 五、环境基线（新设备对齐用）

| 项 | 版本 |
|---|---|
| Rust | 1.99.1 stable-**msvc** |
| MSVC | **14.44.35207** |
| Windows SDK | 10.0.22621.0 / 10.0.26100.0 |
| Node | 22.22.2 |
| WebView2 | 152 |
| NSIS | 3.13（离线包在 `scripts/vendor/`） |

构建耗时基线（热缓存）：`cargo check` 0.84–1.15 s｜`cargo build` debug 76.3 s｜完整打包 **96 s**｜安装包 **21.87 MiB**。

## 六、下一步（到新设备后）

环境通了之后才开始功能开发：**译文悬浮层** → OCR 后自动翻译 → 离线解码提速。
详见 `shotglance/HANDOFF.md` §6。
