# ShotGlance

截图 → OCR → 翻译 → 悬浮回显的 Windows 桌面小工具。

按下 `Ctrl+Shift+S` 框选屏幕任意区域，原文会被立刻识别并翻译，译文以卡片形式贴在原文旁边。
面板关闭后进程继续驻留托盘，内存只有十几 MB。

**Rust + Tauri 2** · 原生 TypeScript 前端 · 本地推理（PP-OCRv4 + ONNX Runtime） · 单文件安装包

---

## 特性

- **框选即译**：框完松手就出译文，不需要切窗口、不需要复制粘贴
- **译文贴原位**：悬浮卡片贴着原文框显示，视线不用移动
- **纯托盘常驻**：空闲 11.5 MB，用完销毁窗口而不是隐藏，WebView2 子进程不常驻
- **本地 OCR**：PP-OCRv4 检测 + 识别，双模型合计 15 MB，不联网也能识别
- **五种翻译引擎**：DeepL / OpenAI 兼容 / Google / MyMemory / 离线 Opus-MT，可在设置里切换；
  `auto` 模式会让多个引擎竞速，谁先返回用谁
- **离线翻译**：Opus-MT en→zh 本地推理，merged decoder + KV cache + int8 词表投影，
  单步 14.84 ms → 2.37 ms
- **单实例**：重复启动不会多开，第二个进程把你已有的窗口叫到前面然后自己退出

## 性能

| 指标 | 目标 | 实测 |
|---|---|---|
| 安装包 | < 150 MB | **21.89 MiB** |
| 空闲内存（托盘） | < 80 MB | 11.5 MB |
| 面板回收后 | < 80 MB | 32.9 MB |
| OCR 峰值 | < 300 MB | 152.7 MB |
| 冷启动到首窗 | < 2 s | 87–112 ms |
| 截图耗时（稳态） | < 300 ms | 130 ms |
| 离线翻译（≤30 token） | < 500 ms | 25–80 ms |

## 安装

从 [Releases](https://github.com/MercuryForever/ShotGlance/releases) 下载
`ShotGlance_x64-setup.exe`，双击安装即可，无需管理员权限。

系统要求：**Windows 10/11 x64**。Windows 11 自带 WebView2；
Windows 10 若缺失，安装包会联网获取运行时。

安装向导支持简体中文与英文，启动时会先弹语言选择框。

## 使用

| 操作 | 效果 |
|---|---|
| `Ctrl+Shift+S` | 打开面板 / 进入框选 |
| 拖动鼠标 | 框选要识别的区域 |
| 右键 / `Esc` | 取消框选 |
| `Esc` | 关闭译文卡片 → 关闭设置 → 关闭面板 |
| 托盘左键 | 打开或前置面板 |
| 托盘右键 | 显示 / 隐藏面板、退出 |

首次使用离线翻译时需要在设置里下载模型（约 113 MB，不含在安装包内）。
没有下载也不影响其他四个在线引擎。**在线引擎按量计费，自动翻译默认开启，
不想产生费用请在设置里关掉或只用离线引擎。**

## 从源码构建

前置：Rust 1.99+（msvc）、Node.js ≥ 20.19、MSVC Build Tools + Windows SDK、NSIS 3.13。

```powershell
.\verify-env.ps1          # 先做 8 项环境自检
.\build.ps1               # 前端 + 后端 + 归档安装包
```

产物落在 `build/`（构建区，不入库）：安装包在 `build\release\`，
中间产物在 `build\target\`。这个目录可以整区删除，重新构建即可重建。

更多细节（离线打包、安装器定制、内存实测口径、调试开关）见
[`shotglance/README.md`](shotglance/README.md)。

## 目录

```
├─ build.ps1            统一构建入口：唯一往 build\ 写东西的脚本
├─ verify-env.ps1       8 项环境自检
├─ CHANGELOG.md         版本记录
└─ shotglance/          工程本体
   ├─ src/              前端（原生 TypeScript，无框架）
   ├─ src-tauri/        后端：capture / ocr / translate / 托盘与窗口管理
   │  ├─ models/        PP-OCRv4 det + rec + 字典（15 MB）
   │  ├─ assets/mt/     离线翻译词表
   │  └─ vendor/        onnxruntime.dll
   └─ scripts/          图标生成、NSIS 缓存、内存实测、OCR 基准图、模型转换
```

编译产物统一进 `build/`，由根 `.gitignore` 的一条 `/build/` 排除。
`vite.config.ts`、`tauri.conf.json`、`.cargo/config.toml` 三处配置把它与源码区连起来。

## 已知限制

- **Windows only**。macOS 需要屏幕录制授权，Linux 无法完全自包含，暂不覆盖。
- **OCR 精度有边界**：中英混排时词间空格偶发丢失，单行置信度低于 0.5 会被丢弃。
- **识别是逐行推理**，整屏 80+ 行时约 8.4 ms/行。日常场景请用框选，不要整屏识别。
- **悬浮卡片是直角方卡**，没有圆角与投影（需要透明窗口，会牺牲部分 DWM 优化）。

## 许可证

[MIT](LICENSE)
