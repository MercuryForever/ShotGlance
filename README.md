# ShotGlance

截图 → OCR → 翻译 → 悬浮回显的轻量 Windows 桌面工具。
**Rust + Tauri 2**，纯托盘常驻，`Ctrl+Shift+S` 框选，译文卡片贴回原位置。

| 指标 | 目标 | v1.0.0 实测 |
|---|---|---|
| 安装包 | < 150 MB | **21.89 MiB** |
| 空闲内存（托盘） | < 80 MB | 11.5 MB |
| 面板回收后 | < 80 MB | 32.9 MB |
| OCR 峰值 | < 300 MB | 152.7 MB |
| 冷启动 | < 2 s | 通过 |

---

## 仓库结构：Git 区 / 构建区

本仓库刻意切成两个互不干涉的区域。**判据只有一条**：凡是可以由源码重新生成的，一律进 `build/`。

```
shotglance/                        ← 仓库根 = Git 区边界
│
├─ 【Git 区】入库，clone 下来就是这些
│  ├─ .gitignore                   唯一定义"什么不进仓库"的地方（一条 /build/ 兜住构建区）
│  ├─ .gitattributes               钉死二进制与换行符（防 dict.txt / tokens.tsv 被 CRLF 破坏）
│  ├─ README.md                    本页：仓库地图与构建方式
│  ├─ CHANGELOG.md                 版本记录（SemVer + Keep a Changelog）
│  ├─ LICENSE                      MIT
│  ├─ build.ps1                    ★ 统一构建入口：唯一往 build\ 写东西的脚本
│  ├─ verify-env.ps1               8 项环境自检（工具链齐不齐）
│  └─ shotglance/                  工程本体
│     ├─ index.html  package.json  tsconfig.json  vite.config.ts
│     ├─ src/                      前端（原生 TS，无框架）
│     ├─ src-tauri/                后端 + 全部构建配置
│     │  ├─ Cargo.toml  Cargo.lock  build.rs
│     │  ├─ .cargo/config.toml     ← 把 target-dir 指到 build/target
│     │  ├─ tauri.conf.json        ← 把 frontendDist 指到 build/dist
│     │  ├─ src/                   lib.rs / capture / ocr / translate
│     │  ├─ models/ assets/ vendor/  运行时资产（onnx、dict、tokens.tsv、ort dll）
│     │  ├─ icons/ capabilities/ tests/
│     │  └─ scripts/               构建与运维脚本（快照、NSIS、内存实测、模型转换）
│     └─ *.md                      交接文档：HANDOFF / ENV-MIGRATION 等
│
└─ 【构建区】不入库，删掉即可全量重建
   └─ build/
      ├─ dist/                    vite 前端产物（index.html + assets/*.js|css）
      ├─ target/                  cargo 编译输出
      │  └─ release/bundle/nsis/   ShotGlance_1.0.0_x64-setup.exe（原始产物）
      ├─ release/                 归档的安装包（清掉 target 后成品仍在）
      └─ logs/                    构建转录（build-YYYYMMDD-HHmmss.log）
```

### 两个区域怎么连起来的

产物能"长"在 `build/` 而不是散落在源码树里，靠三行配置：

| 配置文件（Git 区） | 键 | 值 | 作用 |
|---|---|---|---|
| `shotglance/vite.config.ts` | `build.outDir` | `../build/dist` | 前端产物落到构建区 |
| `shotglance/src-tauri/tauri.conf.json` | `build.frontendDist` | `../../build/dist` | 打包时从这里取前端 |
| `shotglance/src-tauri/.cargo/config.toml` | `build.target-dir` | `../../build/target` | 编译输出落到构建区 |

### 为什么构建配置不放进构建区

用户要的是"区域独立"，但 npm / cargo / vite / tauri 都在**固定的相对路径**上找配置：
`package.json` 必须在工程根、`Cargo.toml` 必须在 `src-tauri/`、`tauri.conf.json` 必须在
`src-tauri/`。把它们挪进 `build/`，工具链立刻找不到工程。

所以职责这样切：

- **配置 = 源码的组成部分**，留在 Git 区，跟着它描述的代码走（进仓库、可 review、可回溯）；
- **脚本 = 构建行为的唯一入口**，`build.ps1` 在 Git 区根目录，只负责"调用工具链 + 把产物写进 `build/`"；
- **产物 = 构建区**，`build/` 整体被 `.gitignore` 的 `/build/` 一条规则排除。

### 例外：`shotglance/src-tauri/gen/`

tauri 会把 ACL / capability 的生成结果写到 `src-tauri/gen/`，路径由工具写死、没有配置项可改。
它同样是"可重建产物"，所以在 `shotglance/.gitignore` 里就地忽略——这是构建区的唯一外溢点。

---

## 构建

```powershell
# 完整构建（前端 + 后端 + 归档安装包）
.\build.ps1

# 只构建前端（tsc --noEmit + vite build）→ build\dist
.\build.ps1 -Target Frontend

# 只构建后端（cargo + NSIS）→ build\target
.\build.ps1 -Target Backend

# 清空构建区后重建 / 出 debug 包
.\build.ps1 -Clean
.\build.ps1 -DebugBuild
```

脚本末尾会打印版本号、profile、产物路径与体积，并把全过程写进 `build\logs\`。

不想用脚本时的等价手工命令：

```powershell
cd shotglance
npm ci
npm run tauri:build
# 产物: ..\build\target\release\bundle\nsis\ShotGlance_1.0.0_x64-setup.exe
```

**前置条件**：Rust 1.99+ (msvc)、Node 22、MSVC BuildTools + Windows SDK、NSIS 3.13（脚本可离线恢复）。
详见 [`shotglance/HANDOFF.md`](shotglance/HANDOFF.md)。

> 构建前务必清掉继承来的代理变量，否则 tauri 下载 NSIS/WebView2 会报 502：
> `foreach ($n in @('http_proxy','https_proxy','all_proxy','no_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY')) { [Environment]::SetEnvironmentVariable($n, $null, 'Process') }`

---

## 文档索引

| 文档 | 讲什么 |
|---|---|
| [`shotglance/README.md`](shotglance/README.md) | 技术决策与实测数据（为什么这么选） |
| [`shotglance/HANDOFF.md`](shotglance/HANDOFF.md) | 环境基线、恢复步骤、不可回退的坑、路线图 |
| [`shotglance/ENV-MIGRATION.md`](shotglance/ENV-MIGRATION.md) | 把开发环境打包带走，下次构建从 3 小时压到 15 分钟 |
| [`shotglance/scripts/mt/README.md`](shotglance/scripts/mt/README.md) | 离线翻译模型的转换流程 |
| [`CHANGELOG.md`](CHANGELOG.md) | 版本变更 |

## 关键资产校验

模型与 dll 体积大、来源固定，换机器后建议比一遍：

| 文件 | 字节 | SHA256 前 16 位 |
|---|---|---|
| `shotglance/src-tauri/models/det.onnx` | 4745517 | `d2a7720d45a54257` |
| `shotglance/src-tauri/models/rec.onnx` | 10857958 | `48fc40f24f6d2a20` |
| `shotglance/src-tauri/models/dict.txt` | 26250 | `a1c84d9bdb9ab290` |
| `shotglance/src-tauri/vendor/onnxruntime.dll` | 19288888 | `68d0d5f502be4509` |
| `shotglance/src-tauri/assets/mt/tokens.tsv` | 1867691 | `e50ef81b726f7d68` |

```powershell
Get-FileHash shotglance\src-tauri\vendor\onnxruntime.dll -Algorithm SHA256
```

> 离线翻译模型（Opus-MT，113 MB）不入仓库也不入安装包，首次使用时由应用下载到
> `%APPDATA%\ShotGlance\models\opus-mt-en-zh\`。

## 许可证

[MIT](LICENSE)
