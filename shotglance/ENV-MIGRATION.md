# ENV-MIGRATION — 怎么保存工作区，让下次构建从 3 小时变成 15 分钟

> 2026-11-12 在一台干净机器上从零配环境，实测耗时约 **3 小时**，其中 **2.5 小时纯粹是下载**。
> 本文档描述如何把那些下载结果固化下来，下次换机器直接搬运。
>
> 配套脚本：`scripts/export-env-bundle.ps1`（导出）、`scripts/restore-env-bundle.ps1`（恢复）。

---

## 1. 先看清那 3 小时花在哪

| 阶段 | 实际耗时 | 瓶颈 | 可缓存？ |
|---|---|---|---|
| 下载 Rust 工具链（250 MB） | 34 分钟 | rust-lang 直连 **71 KB/s**，国内镜像全都没有 `rustup-init` 这个文件 | ✅ 100% |
| 下载 VS BuildTools + SDK（1.9 → 3.2 GB） | ~2 小时 | VS CDN **200 KB/s** | ✅ 100% |
| `cargo check` 拉 crates.io（Tauri 依赖树） | 31 分钟 | 一半是下载，一半是首次全量编译 | ✅ 下载部分 100% |
| `npm ci` | 2 分钟 | — | ✅ |
| 恢复 NSIS 缓存 | 3 秒 | 已由 `scripts/vendor/` 离线解决 | ✅ |
| 安装过程本身（解压 / 注册 / 链接） | ~15 分钟 | 这部分跑不掉 | ❌ |

**结论：能被缓存的是 2.5 小时，跑不掉的是 15 分钟。** 所以目标就是把 2.5 小时压成 15 分钟内的"复制 + 本地安装"。

---

## 2. 三层资产，按需取舍

```
┌─ L0  源码快照 ────────────── 45 MB     export-snapshot.ps1 已有
│  源文件 + 模型 + onnxruntime.dll + tokens.tsv + 成品安装包
│  没有它什么都没有；有了它也只能从零下依赖
│
├─ L1  环境包 ──────────────── 756 MB    export-env-bundle.ps1（本文档新增）
│  ├ rustup 工具链（含 settings.toml）        590.5 MB   ← 省 34 分钟
│  ├ cargo registry cache + index              76.1 MB   ← 省 cargo 的下载时间
│  ├ node_modules                              60.6 MB   ← 省 npm ci
│  └ rustup-init.exe 12.2 + vs_BuildTools.exe 4.3        ← 联网兜底
│     （cargo\bin 只带 rustup.exe 12.2 MB，13 个 shim 恢复时重建硬链接）
│
├─ L1+ target ──────────────── +1.0 GB   同一选项，-WithTarget
│  Rust 编译产物（3305 文件），**仅工作区绝对路径完全一致时有效**
│  省 30 分钟全量编译 → 变成秒级增量
│
└─ L2  VS 离线源 ──────────── +2.5 GB     export-env-bundle.ps1 -WithVsLayout
   VS BuildTools 的 --layout 本地安装源
   省 2 小时下载 → 变成 10-15 分钟本地安装
```

**推荐组合**：L0 + L1 + L1+ + L2 ≈ **4.4 GB**，一个 8 GB U 盘装得下。
下次恢复总时间 **15–20 分钟**，其中 12 分钟是在装 VS。

如果磁盘紧张，砍掉 L1+（1.0 GB）和 L2（2.5 GB），只留 **L0 + L1 = 801 MB**，
下次仍需联网装 VS（2 小时）但 Rust 那边省下来了——不划算，**L2 是最值钱的一档**。

> 实测记录（2026-11-12）：
> - L1（不含 target）= **756 MB**，1371 个文件，导出耗时 **5 秒**（本地 NVMe 同盘 robocopy，约 150 MB/s）
> - L1+ target = **1048 MB**，3305 个文件，导出耗时 **8 秒**
> - 当前 `env-bundle\` 实际总量 **1.8 GB**

---

## 3. 关键前提：固定工作区路径

`target/` 里的 `*.d`（dep-info）文件记录了**绝对路径**。路径一变，cargo 判定全部过期，1.0 GB 白带。

三种处理方式，按推荐度排序：

| 做法 | 说明 |
|---|---|
| **① 下次还放这个路径** | 本次是 `C:\Users\Administrator\Downloads\2026-11-09-08-43-20`。新机器上照抄即可，最省事 |
| **② 放到固定短路径** | 例如统一用 `C:\dev\shotglance`。建议**从现在起就改**，以后不再有路径问题 |
| ③ junction 指过去 | `mklink /J C:\dev\shotglance D:\real\path`，对 cargo 透明，但多一层间接 |

> 恢复脚本会自动比对 MANIFEST 里记录的 `workspace`，不一致就跳过 target 并打印两条路径，不会给你一个"看起来恢复了其实全在重编"的假象。

---

## 4. 导出（当前机器，做一次）

```powershell
cd <工作区>\shotglance

# 最小集：工具链 + 依赖缓存 + node_modules（~750 MB，3 分钟）
powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -OutDir D:\sg-env

# 加上编译产物（+1.0 GB），前提是下次路径一致
powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -OutDir D:\sg-env -WithTarget

# 加上 VS 离线源（+2.5 GB，要下载，网慢就挂一夜）
powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -OutDir D:\sg-env -WithTarget -WithVsLayout
```

同时别忘了源码快照：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\export-snapshot.ps1
```

导出后拿到两样东西：

```
env-bundle\                     ← 目录，含 MANIFEST.txt
  MANIFEST.txt                    版本、每部分体积、安装器 SHA256、原始 workspace 路径
  installers\rustup-init.exe
  installers\vs_BuildTools.exe
  toolchain\rustup\
  toolchain\cargo-bin\
  toolchain\cargo-registry\{cache,index}\
  toolchain\cargo-home\config.toml     （存在才带）
  node_modules\
  target\                              （-WithTarget 才有）
  vs-layout\                           （-WithVsLayout 才有）

shotglance-snapshot-<时间戳>.zip  ← 源码，45 MB
```

> 要传网盘就先压成 zip（`Compress-Archive` 或 7z），590 MB 的 rustup 目录压缩率约 35%。U 盘直传就不必压。

---

## 5. 恢复（新机器）

前提：先把源码快照解压到目标路径，让 `shotglance\` 就位。

```powershell
cd <新工作区>\shotglance

# 常规恢复（Rust 工具链 + cargo 缓存 + node_modules + target 判定）
powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env

# 顺手把 %USERPROFILE%\.cargo\bin 加进用户 PATH，并在最后跑 cargo check 验收
powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -AddToPath -Check

# 如果 bundle 里有 vs-layout，让脚本直接提权装 VS（会弹 UAC）
powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -WithVsInstall
```

脚本会依次做：读 MANIFEST → 恢复 `.rustup` → 恢复 `.cargo\bin` + `registry` → 恢复 `node_modules` → 判定 target → 检查 MSVC/SDK → 检查 PATH → 验证 `rustc -V` / `cargo -V`。

最后再跑一遍项目自带的环境自检（**必须在 `shotglance\` 目录下**）：

```powershell
powershell -ExecutionPolicy Bypass -File ..\verify-env.ps1
```

期望 **0 FAIL**，1 WARN（CMake，走 ort 预编译不需要）。

---

## 6. 没有 vs-layout 时，怎么补 VS（唯一跑不掉的一步）

`vs-layout` 是 VS BuildTools 的官方离线安装源。生成需要联网下载一次，之后永久离线。

```powershell
# 在源机器上生成（约 2.5 GB，把 en-US 限定掉可以少下别的语言包）
& .\installers\vs_BuildTools.exe --layout D:\sg-env\vs-layout `
    --add Microsoft.VisualStudio.Workload.VCTools `
    --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    --includeRecommended --lang en-US --quiet --wait --norestart
```

新机器上从本地装：

```powershell
# 必须管理员。若报 0x80070057，先清代理变量（见 §7 坑 1）
& E:\sg-env\vs-layout\vs_BuildTools.exe --noWeb `
    --add Microsoft.VisualStudio.Workload.VCTools `
    --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    --includeRecommended --quiet --wait --norestart
```

> **不要为了省 1 GB 去砍组件。** 现在这套参数（VCTools + VC.Tools.x86.x64 + recommended）是在本机验证过能编过整个 Tauri 依赖树的组合，砍掉 `--includeRecommended` 会缺 Windows SDK，反而引入新的不确定性。

---

## 7. 五个必须记住的坑

### 坑 1 — VS 安装器 `0x80070057`：代理变量大小写冲突
`HTTP_PROXY` 和 `http_proxy` 同时在环境块里时，安装器建字典撞键，**在启动任何东西之前就退出**，日志只有一行 `Couldn't launch setup process`。
`Start-Process -Verb RunAs` 的子进程**继承调用者环境块**，不会给你干净环境。解法必须在**发起提权的那一个进程**里清：

```powershell
foreach ($n in @('http_proxy','https_proxy','all_proxy','no_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY')) {
  [Environment]::SetEnvironmentVariable($n, $null, 'Process')
}
Start-Process -FilePath $vsBoot -ArgumentList $layArgs -Verb RunAs -Wait
```

### 坑 2 — `Start-Process ... -Verb RunAs` 不加 `-Wait` 会被回收
本次第一次装 VS 就死在这：`Start-Process` 一返回，会话结束，沙箱回收进程树，日志精确停在那次调用返回的时刻。
**必须 `-Wait`，且放在后台任务里跑**（一次要 1-2 小时）。

### 坑 3 — Build Tools 装在 `Program Files (x86)`
不是 `Program Files`。判断装没装**只用 vswhere**，别硬猜路径：

```powershell
& 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' `
  -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
```

### 坑 4 — `verify-env.ps1` 的两个隐含前提
① 必须在 `shotglance\` 目录下跑（它用 `Get-Location` 找 `node_modules\.bin\tauri.cmd`）；
② PATH 里要有 `%USERPROFILE%\.cargo\bin`（它用 `Get-Command rustc` 找编译器）。
缺任意一条都会误报 3 项 FAIL，看起来像环境没配好。

### 坑 5 — `.ps1` 要么纯 ASCII，要么 UTF-8 **带 BOM**
PS 5.1 把无 BOM 的 UTF-8 当 ANSI(GBK) 读，中文注释会破坏 `param` 块并**静默丢参数**。
本次 `verify-env.ps1` 迁移后 BOM 丢失，跑起来一片 ParserError。
两个新脚本因此写成**纯 ASCII**，不需要 BOM 也不会有这个问题。

---

## 8. 恢复后自检清单

| 检查 | 期望 |
|---|---|
| `rustc -V` | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` 或 MANIFEST 记录值 |
| `cargo -V` | `cargo 1.99.1 (5f94df478 2026-08-27)` |
| `vswhere ... -property installationVersion` | `17.14.x` |
| MSVC 目录名 | `14.44.35207` |
| `Windows Kits\10\Include` | 含 `10.0.26100.0` |
| `%LOCALAPPDATA%\tauri\NSIS` | 443 个文件 / 6.9 MB（跑 `scripts\restore-nsis-cache.ps1`） |
| `shotglance\node_modules\.bin\tauri.cmd` | 存在 |
| `verify-env.ps1`（在 `shotglance\` 下） | 0 FAIL / 1 WARN |
| `cargo check` | Finished，首次若复用 target 应 < 1 分钟 |
| 模型资产校验和 | 见 `HANDOFF.md` §3.3 |

---

## 8.5 这套方案做过哪些验证（2026-11-12 实测）

不是"写完就交"。导出包在 `%TEMP%` 下做过一次完整沙盒恢复，用 `RUSTUP_HOME` / `CARGO_HOME` 指向沙盒目录隔离，不污染本机环境：

| 验证项 | 方法 | 结果 |
|---|---|---|
| bundle 结构完整 | 导出后比对文件数 / 体积 / MANIFEST 段 | ✅ L1 1371 文件 756 MB（+target 后 4676 文件 1.8 GB），5 个 part 全在 |
| 工具链可搬移 | 把 `toolchain\rustup` 复制到全新目录，设 `RUSTUP_HOME` 指过去 | ✅ `rustc 1.99.1`、`rustup show` 认出 default toolchain |
| shim 硬链接可行 | 只用 `rustup.exe` + `New-Item -ItemType HardLink` 重建 13 个名字 | ✅ 13/13 成功，磁盘只占 1 份（12.2 MB 而非 172 MB） |
| cargo 缓存完整 | `cargo metadata --offline --filter-platform x86_64-pc-windows-msvc` | ✅ PASS，15.4 秒，651 个依赖行 |
| 编译链路可用 | 沙盒环境里 `cargo run` 一个 hello world | ✅ 输出 `linker ok`，产物 131.5 KB |
| 导出脚本统计 | 修 `Measure-Object` 缺 `-Property Length -Sum` 的 bug 后重跑 | ✅ 各 part 体积正确显示 |

**踩到的两个坑（已在脚本里修掉）**：

1. **`%USERPROFILE%\.cargo\bin` 里 13 个 shim 是指向 `rustup.exe` 的符号链接**。robocopy 会跟随链接复制成 14 份实体文件 = 172 MB，白胖 163 MB。现在只带 `rustup.exe` + 把 shim 名字写进 MANIFEST，恢复时用硬链接重建（同卷、不需要管理员）。
2. **`cargo metadata --offline` 不带平台过滤会失败**，报缺 `android_system_properties`。原因是它要解析所有平台的依赖，而我们只编译过 windows，cache 里自然没有 android 的包。**这不是缓存不全**，加 `--filter-platform x86_64-pc-windows-msvc` 就 PASS。

---

## 9. 备份策略（按保底程度排）

| 方案 | 体积 | 恢复时间 | 适用 |
|---|---|---|---|
| **本文档的 env-bundle** | 4.4 GB | 15–20 分钟 | 换设备首选，跨机器安全 |
| 源码快照 alone | 45 MB | 3 小时 | 只要代码，能忍下载 |
| 全盘镜像（Macrium / 系统映像） | 40–80 GB | 30 分钟 | 同机重装最佳；**跨硬件会蓝屏**，不适用于换机器 |
| Git 仓库 + 本文档 | ~50 MB | 3 小时 + 排错 | 长期看最好的工程实践，但救不了这次这种"环境从零"场景 |

> 实际操作建议：**三样都做**——`git init` 管代码演进，快照 zip 管资产，env-bundle 管环境。三者互不替代。

---

## 10. 一句话流程

```powershell
# 旧机器（一次性）
powershell -ExecutionPolicy Bypass -File scripts\export-env-bundle.ps1 -OutDir E:\sg-env -WithTarget -WithVsLayout
powershell -ExecutionPolicy Bypass -File scripts\export-snapshot.ps1

# 新机器（15 分钟）
# ① 解压源码快照到 C:\Users\Administrator\Downloads\2026-11-09-08-43-20
powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -AddToPath -WithVsInstall -Check
powershell -ExecutionPolicy Bypass -File ..\verify-env.ps1
```
