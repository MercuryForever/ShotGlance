# 工作区索引 — ShotGlance

> 换设备 / 隔很久回来，先读这一页，再按需跳到具体文档。
> 最后整理：2026-11-12

## 这是什么

一个桌面小工具 **ShotGlance** 的全部工程：截图 → OCR → 翻译 → 悬浮回显。
Rust + Tauri 2，纯托盘常驻，`Ctrl+Shift+S` 唤起，Windows 专属。

| 文档 | 讲什么 | 什么时候看 |
|---|---|---|
| **本页** | 工作区地图、迁移该带什么 | 每次回来 |
| [`shotglance/ENV-MIGRATION.md`](shotglance/ENV-MIGRATION.md) | **怎么把环境打包带走**，把下次构建从 3 小时压到 15 分钟 | 换设备前 |
| [`shotglance/HANDOFF.md`](shotglance/HANDOFF.md) | 环境基线、恢复步骤、不可回退的坑、路线图 | 在新机器上装环境 |
| [`shotglance/README.md`](shotglance/README.md) | 技术决策与实测数据（为什么这么选） | 改架构前 |
| [`shotglance/scripts/mt/README.md`](shotglance/scripts/mt/README.md) | 离线翻译模型的转换流程 | 换语言对/换模型 |
| `.workbuddy/memory/` | 逐日踩坑流水账 + 长期结论 | 想不起某个坑怎么踩的 |

## 目录地图

```
2026-11-09-08-43-20\
├─ README.md                    ← 本页
├─ verify-env.ps1               8 项环境自检（工具链齐不齐）
├─ shotglance-snapshot-*.zip    可迁移快照（白名单打包，含成品安装包）
├─ shotglance-snapshot-*-report.txt   快照清单（含/不含什么）
├─ env-bundle\                  ★ 环境包 1.8 GB（含 target，让下次构建只要 15 分钟）
│  ├─ MANIFEST.txt              版本 + 各部分体积 + 安装器 SHA256 + 原始路径
│  ├─ installers\               rustup-init.exe / vs_BuildTools.exe
│  ├─ toolchain\                rustup 工具链 590 MB + cargo 缓存 76 MB
│  ├─ node_modules\             60 MB
│  ├─ target\                   （-WithTarget 才有，+1.0 GB，仅路径一致时有用）
│  └─ vs-layout\                （-WithVsLayout 才有，+2.5 GB，VS 离线安装源）
└─ shotglance\                  工程本体
   ├─ HANDOFF.md  README.md  ENV-MIGRATION.md  package.json  tsconfig.json
   ├─ index.html                 面板骨架
   ├─ src\                       前端（原生 TS，无框架）
   │  ├─ main.ts                 940 行，invoke 通道 + 交互
   │  └─ styles.css
   ├─ release\                   ★ 已构建的安装包（可直接分发）
   │  └─ ShotGlance_0.1.0_x64-setup.exe   22 MB
   ├─ src-tauri\
   │  ├─ src\                    后端 3 个模块，约 3600 行
   │  │  ├─ lib.rs               961 行：托盘、热键、建窗/回收、选区形态、命令注册、自检入口
   │  │  ├─ capture\mod.rs       xcap/WGC 采集、区域裁剪、JPEG 编码、预热
   │  │  ├─ ocr\                 ort 会话管理 + DB 检测 + CTC 识别
   │  │  └─ translate\           翻译层（Rust 模块名 mt）
   │  │     ├─ mod.rs            配置、引擎注册表、对冲竞速、失败熔断
   │  │     ├─ online.rs         DeepL / OpenAI 兼容 / Google / MyMemory
   │  │     └─ offline.rs        词表 + Unigram Viterbi + ONNX 编解码
   │  ├─ models\                 PP-OCRv4 det/rec + 6623 字字典（15 MB，必带）
   │  ├─ assets\mt\tokens.tsv    65001 行 id/score/piece（1.8 MB，必带）
   │  ├─ vendor\onnxruntime.dll  ORT 1.31.1（19 MB，必带，不在系统 PATH 上）
   │  ├─ tests\ocr-sample.png    OCR 基准图（8 行，验证回归用）
   │  ├─ capabilities\ icons\    权限白名单、图标
   │  ├─ tauri.conf.json         resources 映射、NSIS 配置
   │  └─ Cargo.toml / Cargo.lock ort 锁 =2.0.0-rc.15，lock 必须带
   └─ scripts\
      ├─ export-snapshot.ps1     白名单打包（新增文件记得加进列表）
      ├─ export-env-bundle.ps1   ★ 导出环境包（工具链 + 依赖缓存 + node_modules）
      ├─ restore-env-bundle.ps1  ★ 在新机器上恢复环境包
      ├─ restore-nsis-cache.ps1  离线恢复 NSIS（断网打包的关键）
      ├─ measure-memory.ps1      三阶段内存实测
      ├─ make-ocr-sample.ps1     重画基准图
      ├─ gen_icon.py             重生成图标（纯 stdlib）
      ├─ mt\                     离线翻译模型的转换脚本（不参与构建）
      └─ vendor\                 NSIS 3.13 + 插件 dll（2.4 MB，必带）
```

## 换设备：四条路径

### 路径 A — 只想用，不改代码（30 秒）

直接拿 `shotglance\release\ShotGlance_0.1.0_x64-setup.exe` 双击装。
`currentUser` 安装，免管理员。离线翻译模型首次使用时应用内下载（113 MB）。

### 路径 B — 要能继续开发，且带了 `env-bundle\`（15–20 分钟）★ 推荐

2026-11-12 实测：从零配环境要 **3 小时，其中 2.5 小时是纯下载**。`env-bundle\` 就是把那 2.5 小时的落盘结果。

```powershell
# ① 解压源码快照到目标路径（尽量沿用记录里的同一路径，target 才能复用）
# ② 恢复环境
cd <工作区>\shotglance
powershell -ExecutionPolicy Bypass -File scripts\restore-env-bundle.ps1 -BundleDir E:\sg-env -AddToPath -WithVsInstall -Check
# ③ 自检
powershell -ExecutionPolicy Bypass -File ..\verify-env.ps1
```

包含：rustup 工具链、cargo crate 缓存、node_modules、两个引导器 exe；
可选 `target`（+1.0 GB，需路径一致）和 `vs-layout`（+2.5 GB，VS 离线装）。
细节与踩坑见 [`shotglance/ENV-MIGRATION.md`](shotglance/ENV-MIGRATION.md)。

### 路径 C — 要能继续开发，但只有源码快照（约 3 小时）

1. 把 `shotglance-snapshot-*.zip` 拷到新机器，解压。
2. 按 [`HANDOFF.md` §4](shotglance/HANDOFF.md) 装工具链、自检、`npm ci`、`npm run tauri:dev`。

快照是**白名单**打包，代码 + 模型 + dll + 脚本 + 文档 + 成品安装包全在内，共 55 个文件、约 45 MB。
`node_modules`、`target`、`dist` 这些可重建的都不在。

### 路径 D — 手头只有这个工作区目录（没有 zip）

等价于路径 C，只是不用解压。目录已经清理过，体积主要就是模型和 dll。
缺点是少了 `MANIFEST.txt` 的逐文件校验和——用下面这张表核对关键资产即可。

## 关键资产校验（到新机器上比一遍）

| 文件 | 字节 | SHA256 前 16 位 |
|---|---|---|
| `src-tauri/models/det.onnx` | 4745517 | `d2a7720d45a54257` |
| `src-tauri/models/rec.onnx` | 10857958 | `48fc40f24f6d2a20` |
| `src-tauri/models/dict.txt` | 26250 | `a1c84d9bdb9ab290` |
| `src-tauri/vendor/onnxruntime.dll` | 19288888 | `68d0d5f502be4509` |
| `src-tauri/assets/mt/tokens.tsv` | 1867691 | `e50ef81b726f7d68` |

```powershell
Get-FileHash shotglance\src-tauri\vendor\onnxruntime.dll -Algorithm SHA256
```

## 哪些该带、哪些别带

默认不带（可重建）：

| 路径 | 体积 | 重建方式 |
|---|---|---|
| `shotglance/src-tauri/target` | 1.0 GB（release 全量曾到 7.6 GB） | `cargo check` / `npm run tauri:build` |
| `shotglance/node_modules` | 62 MB | `npm ci` |
| `shotglance/dist` | 204 KB | `npm run build` |
| `shotglance/src-tauri/gen` | 313 KB | `tauri-build` 自动生成 |
| `.setup` | 19 MB | 官方渠道重新下载（NSIS 已另存 `scripts/vendor/`） |

**例外**：走 `env-bundle` 时，`node_modules` 和 `target` 就**值得带**——
它们省 2 分钟 npm ci 和 30 分钟全量编译。`target` 的唯一前提是工作区绝对路径不变，
否则 cargo 会因为 dep-info 里的路径对不上而全部重编（详见 `ENV-MIGRATION.md` §3）。

## 当前状态（一句话版）

骨架 / 托盘 / 热键 / 截屏 / OCR / 区域框选 / 翻译层（5 引擎 + 离线）都已完成并打包验证，**只差译文悬浮层**。
硬指标全部达标：装包 21.91 MiB、托盘 11.5 MB、回收后 32.9 MB、冷启动 <2 s、截图 130 ms。

下一步顺序：译文悬浮层 → OCR 后自动翻译 → 离线解码提速。详见 `HANDOFF.md` §6。

## 工作区与构建环境（2026-11-13 落地）

| 项 | 状态 |
|---|---|
| 目录 | 已扁平化为单层，工程在 `shotglance\`，与 target 缓存的绝对路径记录一致 |
| 版本库 | **已是 git 仓库**，首提交 `8815d52`（58 文件 / 39.8 MB） |
| Rust 工具链 | 1.99.1 msvc，已从 `env-bundle` 恢复，PATH 已配 |
| 构建缓存 | `target` 3305 文件 / 1048 MB 已恢复，`cargo check` **0.84–1.15 s** |
| 前端 | `tsc --noEmit` 0 错 |
| **MSVC + Win SDK** | **缺失（唯一硬阻塞）**——`cargo check` 可跑，`build` / `tauri:build` 不行 |

装编译器的唯一办法（约 2 小时 + UAC，先把重复代理变量清掉否则 `0x80070057`）：

```powershell
env-bundle\installers\vs_BuildTools.exe --add Microsoft.VisualStudio.Workload.VCTools `
  --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --includeRecommended --quiet --wait --norestart
```

`verify-env.ps1` 另报 Python 3.10/3.11 缺失，那只影响重新导出模型，不影响日常构建。
