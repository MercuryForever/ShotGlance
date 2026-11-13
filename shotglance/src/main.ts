import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface MonitorInfo {
  index: number;
  name: string;
  width: number;
  height: number;
  scale: number;
  primary: boolean;
}

interface CaptureResult {
  monitor: number;
  width: number;
  height: number;
  capture_ms: number;
  encode_ms: number;
  total_ms: number;
  jpeg_b64: string;
}

interface OcrLine {
  text: string;
  conf: number;
  x: number;
  y: number;
  w: number;
  h: number;
}

interface OcrResult {
  text: string;
  lines: OcrLine[];
  det_ms: number;
  rec_ms: number;
  total_ms: number;
  det_size: [number, number];
}

interface OcrScreen {
  monitor: number;
  width: number;
  height: number;
  capture_ms: number;
  encode_ms: number;
  ocr: OcrResult;
  jpeg_b64: string;
}

/** 进入选区时后端返回的冻结帧。 */
interface RegionBegin {
  monitor: number;
  width: number;
  height: number;
  capture_ms: number;
  jpeg_b64: string;
}

/** 框选完成后的裁剪结果（ocr 为 null 表示只截图没识别）。 */
interface RegionResult {
  monitor: number;
  x: number;
  y: number;
  width: number;
  height: number;
  capture_ms: number;
  crop_ms: number;
  encode_ms: number;
  jpeg_b64: string;
  /** 选区在屏幕坐标系里的原点（物理像素）。悬浮卡片靠它贴到原文框旁边。 */
  screen_x: number;
  screen_y: number;
  ocr: OcrResult | null;
}

/** 引擎清单项。ready=false 时 reason 说明为什么不能用。 */
interface EngineInfo {
  id: string;
  label: string;
  kind: string;
  ready: boolean;
  reason: string | null;
  needs_config: boolean;
}

/** 与后端 mt::Cfg 一一对应，字段名保持 snake_case。 */
interface MtCfg {
  engine: string;
  source: string;
  target: string;
  mymemory_email: string;
  deepl_key: string;
  deepl_pro: boolean;
  openai_base: string;
  openai_key: string;
  openai_model: string;
  offline_max_tokens: number;
  /** 框选识别完成后自动翻译并悬浮显示。关掉就回到手动点「翻译」。 */
  auto_translate: boolean;
}

interface Outcome {
  text: string;
  engine: string;
  engine_label: string;
  ms: number;
  fell_back_from: string | null;
  note: string | null;
  cached: boolean;
}

interface ModelStatus {
  installed: boolean;
  dir: string;
  bytes: number;
}

interface MtProgress {
  file: string;
  done: number;
  total: number;
  overall: number;
  overall_total: number;
}

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel);

const previewHint = $<HTMLElement>("#preview-hint");
const shotsEl = $<HTMLElement>("#shots");
const shotEl = $<HTMLImageElement>("#shot");
const boxesEl = $<HTMLElement>("#boxes");
const capMetaEl = $<HTMLElement>("#cap-meta");
const ocrMetaEl = $<HTMLElement>("#ocr-meta");
const monitorSel = $<HTMLSelectElement>("#monitor");
const captureBtn = $<HTMLButtonElement>("#capture");
const ocrBtn = $<HTMLButtonElement>("#ocr");
const ocrEl = $<HTMLElement>("#ocr-text");
const transEl = $<HTMLElement>("#trans-text");
const runBtn = $<HTMLButtonElement>("#run");
const regionBtn = $<HTMLButtonElement>("#region");

const selector = $<HTMLElement>("#selector");
const selBg = $<HTMLImageElement>("#sel-bg");
const selBox = $<HTMLElement>("#sel-box");
const selSize = $<HTMLElement>("#sel-size");
const selVLine = $<HTMLElement>("#sel-vline");
const selHLine = $<HTMLElement>("#sel-hline");

/* 翻译相关。设置面板是覆盖层，元素在这一层统一取好。 */
const transMetaEl = $<HTMLElement>("#trans-meta");
const engineSel = $<HTMLSelectElement>("#engine");
const cfgOpenBtn = $<HTMLButtonElement>("#cfg-open");
const cfgEl = $<HTMLElement>("#cfg");
const cfgCloseBtn = $<HTMLButtonElement>("#cfg-close");
const cfgEngineSel = $<HTMLSelectElement>("#cfg-engine");
const cfgEngineHint = $<HTMLElement>("#cfg-engine-hint");
const cfgSourceSel = $<HTMLSelectElement>("#cfg-source");
const cfgTargetSel = $<HTMLSelectElement>("#cfg-target");
const cfgDeplKey = $<HTMLInputElement>("#cfg-deepl-key");
const cfgDeplPro = $<HTMLInputElement>("#cfg-deepl-pro");
const cfgBase = $<HTMLInputElement>("#cfg-openai-base");
const cfgKey = $<HTMLInputElement>("#cfg-openai-key");
const cfgModel = $<HTMLInputElement>("#cfg-openai-model");
const cfgMmEmail = $<HTMLInputElement>("#cfg-mm-email");
const cfgMtStatus = $<HTMLElement>("#cfg-mt-status");
const cfgMtDl = $<HTMLButtonElement>("#cfg-mt-dl");
const cfgMtDir = $<HTMLButtonElement>("#cfg-mt-dir");
const cfgDlBar = $<HTMLElement>("#cfg-dl-bar");
const cfgDlFill = $<HTMLElement>("#cfg-dl-fill");
const cfgTestBtn = $<HTMLButtonElement>("#cfg-test");
const cfgSaveBtn = $<HTMLButtonElement>("#cfg-save");
const cfgOut = $<HTMLElement>("#cfg-out");

/* 悬浮卡片。它的 DOM 和面板共用同一个窗口，只是形态不同。 */
const ovEl = $<HTMLElement>("#ov");
const ovHeadEl = $<HTMLElement>("#ov-head");
const ovMetaEl = $<HTMLElement>("#ov-meta");
const ovTextEl = $<HTMLElement>("#ov-text");
const ovBackBtn = $<HTMLButtonElement>("#ov-back");
const ovCopyBtn = $<HTMLButtonElement>("#ov-copy");
const cfgAutoTrans = $<HTMLInputElement>("#cfg-auto-trans");

let lastText = "";

/** 选区底图的像素尺寸（物理像素）。裁剪坐标最终要按它换算。 */
let frameW = 0;
let frameH = 0;

/** 是否处在选区形态。Esc、按钮互斥都靠它判断。 */
let selecting = false;

/** 选区已提交、正在裁剪识别。这段时间窗口已还原成面板，
 *  但结果还没回来——此时按 Esc 不能当成"关闭面板"，否则刚框出来的结果直接丢了。 */
let regionBusy = false;

/** 拖动起点（相对选区层的 CSS 像素）。null 表示还没按下。 */
let dragFrom: { x: number; y: number } | null = null;

/** 小于这个边长（CSS 像素）视为误触，不提交。 */
const MIN_SELECT_PX = 6;

/** 后端返回的像素坐标 → 百分比，交给 CSS 定位。 */
function renderBoxes(lines: OcrLine[], imgW: number, imgH: number): void {
  if (!boxesEl) return;
  boxesEl.innerHTML = "";
  for (const l of lines) {
    const d = document.createElement("div");
    d.className = l.conf < 0.75 ? "box low" : "box";
    d.style.left = `${(l.x / imgW) * 100}%`;
    d.style.top = `${(l.y / imgH) * 100}%`;
    d.style.width = `${(l.w / imgW) * 100}%`;
    d.style.height = `${(l.h / imgH) * 100}%`;
    d.title = `${l.text}（${l.conf.toFixed(2)}）`;
    // 点框复制该行：翻译整页时常只需要其中一句。
    d.addEventListener("click", () => {
      void navigator.clipboard.writeText(l.text);
      if (ocrMetaEl) ocrMetaEl.textContent = `已复制：${l.text.slice(0, 18)}`;
    });
    boxesEl.appendChild(d);
  }
}

function showShot(b64: string): void {
  if (!shotEl || !shotsEl || !previewHint) return;
  shotEl.src = `data:image/jpeg;base64,${b64}`;
  shotsEl.hidden = false;
  previewHint.hidden = true;
}

async function loadMonitors(selected?: number): Promise<void> {
  if (!monitorSel) return;
  try {
    const list = await invoke<MonitorInfo[]>("list_monitors");
    monitorSel.innerHTML = "";
    for (const m of list) {
      const opt = document.createElement("option");
      opt.value = String(m.index);
      opt.textContent = `${m.name} · ${m.width}×${m.height}${m.scale !== 1 ? ` @${m.scale}x` : ""}${m.primary ? " (主)" : ""}`;
      monitorSel.appendChild(opt);
    }
    if (selected !== undefined) {
      monitorSel.value = String(selected);
    } else {
      const primary = list.find((m) => m.primary);
      if (primary) monitorSel.value = String(primary.index);
    }
  } catch (err) {
    if (capMetaEl) capMetaEl.textContent = `枚举显示器失败：${String(err)}`;
  }
}

function selectedMonitor(): number | null {
  const raw = monitorSel?.value ?? "";
  return raw === "" ? null : Number(raw);
}

async function capture(): Promise<void> {
  if (!capMetaEl) return;
  capMetaEl.textContent = "采集中…";
  if (ocrBtn) ocrBtn.disabled = true;

  try {
    const t0 = performance.now();
    const r = await invoke<CaptureResult>("capture_screen", { monitor: selectedMonitor() });
    // 往返时间 = 后端采集+编码 + IPC + 前端解码，这才是用户实际等的时间。
    const roundTrip = Math.round(performance.now() - t0);

    showShot(r.jpeg_b64);
    if (boxesEl) boxesEl.innerHTML = "";
    capMetaEl.textContent = `${r.width}×${r.height} · 采集 ${r.capture_ms} · 编码 ${r.encode_ms} · 往返 ${roundTrip} ms`;

    if (ocrEl) {
      ocrEl.classList.add("placeholder");
      ocrEl.textContent = "已截图，点击「识别」开始 OCR";
    }
    if (ocrMetaEl) ocrMetaEl.textContent = "OCR 未运行";

    // 首次截图成功后才填充显示器列表。
    // 枚举显示器会初始化 D3D 设备（约 42 MB 且不可释放），
    // 不该让"只是打开面板看一眼"的用户付这个成本。
    if (monitorSel && monitorSel.options.length === 0) {
      void loadMonitors(r.monitor);
    }
  } catch (err) {
    capMetaEl.textContent = `失败：${String(err)}`;
  } finally {
    if (ocrBtn) ocrBtn.disabled = false;
  }
}

async function runOcr(): Promise<void> {
  if (!ocrEl || !ocrMetaEl || !capMetaEl) return;
  ocrMetaEl.textContent = "识别中…";
  if (captureBtn) captureBtn.disabled = true;

  try {
    const t0 = performance.now();
    const r = await invoke<OcrScreen>("ocr_screen", { monitor: selectedMonitor() });
    const roundTrip = Math.round(performance.now() - t0);

    // 预览与识别来自同一帧，框不会错位。
    showShot(r.jpeg_b64);
    renderBoxes(r.ocr.lines, r.width, r.height);

    capMetaEl.textContent = `${r.width}×${r.height} · 采集 ${r.capture_ms} · 编码 ${r.encode_ms}`;
    ocrMetaEl.textContent = `${r.ocr.lines.length} 行 · 检测 ${r.ocr.det_ms} · 识别 ${r.ocr.rec_ms} · 合计 ${r.ocr.total_ms} ms（往返 ${roundTrip}）`;

    lastText = r.ocr.text;
    ocrEl.classList.toggle("placeholder", !lastText);
    ocrEl.textContent = lastText || "未识别到文字（可换更大字号或调整区域）";

    if (monitorSel && monitorSel.options.length === 0) {
      void loadMonitors(r.monitor);
    }
  } catch (err) {
    ocrMetaEl.textContent = "识别失败";
    ocrEl.classList.remove("placeholder");
    ocrEl.textContent = String(err);
  } finally {
    if (captureBtn) captureBtn.disabled = false;
  }
}

/* ---------------- 翻译 ---------------- */

/** 最近一次从后端读到的配置。保存时以它为底稿，避免把未展示的字段清空。 */
let mtCfg: MtCfg | null = null;
let engines: EngineInfo[] = [];

const bytesText = (n: number): string =>
  n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${Math.round(n / 1024)} KB`;

/** 填充引擎下拉。auto 不是引擎而是策略，所以单独放在最前面。 */
function fillEngineSelect(sel: HTMLSelectElement | null, cfg: MtCfg): void {
  if (!sel) return;
  sel.innerHTML = "";

  const auto = document.createElement("option");
  auto.value = "auto";
  auto.textContent = "自动（在线优先，失败转离线）";
  sel.appendChild(auto);

  for (const e of engines) {
    const o = document.createElement("option");
    o.value = e.id;
    o.textContent = e.ready ? e.label : `${e.label} · 未就绪`;
    if (!e.ready && e.reason) o.title = e.reason;
    sel.appendChild(o);
  }
  sel.value = cfg.engine;
}

/** 只显示当前引擎需要的字段。auto 显示离线面板：离线是 auto 的兜底，
 *  用户很可能就是来下载模型的。 */
function showEnginePane(engine: string): void {
  const target = engine === "auto" ? "offline" : engine;
  document.querySelectorAll<HTMLElement>(".eng-pane").forEach((p) => {
    p.hidden = p.dataset.eng !== target;
  });

  if (cfgEngineHint) {
    if (engine === "auto") {
      cfgEngineHint.textContent =
        "自动：并行发起可用的在线引擎（每 250 ms 补一个），谁先返回用谁；全部失败后走本地模型。";
    } else {
      const e = engines.find((x) => x.id === engine);
      cfgEngineHint.textContent = e
        ? e.ready
          ? `${e.label} · 可用`
          : `${e.label} · ${e.reason ?? "不可用"}`
        : "";
    }
  }
}

function applyCfg(cfg: MtCfg): void {
  mtCfg = cfg;
  fillEngineSelect(engineSel, cfg);
  fillEngineSelect(cfgEngineSel, cfg);
  if (cfgSourceSel) cfgSourceSel.value = cfg.source;
  if (cfgTargetSel) cfgTargetSel.value = cfg.target;
  if (cfgDeplKey) cfgDeplKey.value = cfg.deepl_key;
  if (cfgDeplPro) cfgDeplPro.checked = cfg.deepl_pro;
  if (cfgBase) cfgBase.value = cfg.openai_base;
  if (cfgKey) cfgKey.value = cfg.openai_key;
  if (cfgModel) cfgModel.value = cfg.openai_model;
  if (cfgMmEmail) cfgMmEmail.value = cfg.mymemory_email;
  if (cfgAutoTrans) cfgAutoTrans.checked = cfg.auto_translate;
  showEnginePane(cfg.engine);
}

/** 把表单读回配置对象。以已有配置为底稿。 */
function readCfg(): MtCfg {
  const base: MtCfg = mtCfg ?? {
    engine: "auto",
    source: "auto",
    target: "zh-CN",
    mymemory_email: "",
    deepl_key: "",
    deepl_pro: false,
    openai_base: "https://api.openai.com/v1",
    openai_key: "",
    openai_model: "gpt-4o-mini",
    offline_max_tokens: 96,
    auto_translate: true,
  };
  return {
    ...base,
    engine: cfgEngineSel?.value ?? base.engine,
    source: cfgSourceSel?.value ?? base.source,
    target: cfgTargetSel?.value ?? base.target,
    deepl_key: cfgDeplKey?.value.trim() ?? base.deepl_key,
    deepl_pro: cfgDeplPro?.checked ?? base.deepl_pro,
    openai_base: cfgBase?.value.trim() ?? base.openai_base,
    openai_key: cfgKey?.value.trim() ?? base.openai_key,
    openai_model: cfgModel?.value.trim() ?? base.openai_model,
    mymemory_email: cfgMmEmail?.value.trim() ?? base.mymemory_email,
    auto_translate: cfgAutoTrans?.checked ?? base.auto_translate,
  };
}

async function saveCfg(silent = false): Promise<void> {
  const cfg = readCfg();
  try {
    await invoke("translate_cfg_set", { cfg });
    mtCfg = cfg;
    if (engineSel) engineSel.value = cfg.engine;
    engines = await invoke<EngineInfo[]>("translate_engines");
    showEnginePane(cfg.engine);
    if (!silent && cfgOut) cfgOut.textContent = "已保存";
  } catch (err) {
    if (cfgOut) cfgOut.textContent = `保存失败：${String(err)}`;
  }
}

async function refreshMtStatus(): Promise<void> {
  if (!cfgMtStatus) return;
  try {
    const s = await invoke<ModelStatus>("mt_status");
    cfgMtStatus.textContent = s.installed
      ? `已安装 · ${bytesText(s.bytes)} · ${s.dir}`
      : "未安装（选它之前需要先下载）";
    if (cfgMtDl) cfgMtDl.disabled = s.installed;
    if (cfgMtDl) cfgMtDl.textContent = s.installed ? "已下载" : "下载模型（约 113 MB）";
    cfgMtStatus.title = s.dir;
  } catch (err) {
    cfgMtStatus.textContent = `状态读取失败：${String(err)}`;
  }
}

async function downloadModel(): Promise<void> {
  if (!cfgMtDl) return;
  cfgMtDl.disabled = true;
  cfgMtDl.textContent = "下载中…";
  if (cfgDlBar) cfgDlBar.hidden = false;
  if (cfgOut) cfgOut.textContent = "";

  try {
    await invoke("mt_download");
    if (cfgOut) cfgOut.textContent = "模型下载完成";
  } catch (err) {
    if (cfgOut) cfgOut.textContent = `下载失败：${String(err)}`;
  } finally {
    if (cfgDlBar) cfgDlBar.hidden = true;
    if (cfgDlFill) cfgDlFill.style.width = "0%";
    await refreshMtStatus();
    if (cfgOut && !cfgMtDl.disabled) cfgMtDl.textContent = "下载模型（约 113 MB）";
  }
}

async function testEngine(): Promise<void> {
  if (!cfgTestBtn || !cfgOut) return;
  // 先落盘：用户改了 key 直接点测试是极自然的动作，
  // 不先保存的话测的是旧配置，会让人以为 key 无效。
  await saveCfg(true);
  const id = cfgEngineSel?.value ?? "auto";

  cfgTestBtn.disabled = true;
  cfgOut.textContent = "试译中…";

  try {
    const r = await invoke<Outcome>("translate_test", { engine: id });
    cfgOut.textContent = `${r.engine_label} · ${r.ms} ms · ${r.text}`;
  } catch (err) {
    cfgOut.textContent = `失败：${String(err).split("\n")[0]}`;
    cfgOut.title = String(err);
  } finally {
    cfgTestBtn.disabled = false;
  }
}

/** 一次翻译结果的副标题（引擎 · 耗时 · 是否缓存 · 从谁降级而来）。 */
function outcomeMeta(r: Outcome): string {
  const parts = [r.engine_label, `${r.ms} ms`];
  if (r.cached) parts.push("缓存");
  if (r.fell_back_from) parts.push(`降级自 ${r.fell_back_from}`);
  return parts.join(" · ");
}

/** 把翻译结果落到面板的译文区。
 *
 * 自动翻译（框选后）走的也是这条路径：用户按「返回」收起悬浮卡片时，
 * 面板里必须已经有译文，否则"关掉悬浮"就等于把刚翻出来的东西丢了。 */
function renderOutcome(r: Outcome): void {
  if (transEl) {
    transEl.classList.toggle("placeholder", !r.text);
    transEl.textContent = r.text || "（无译文）";
  }
  if (transMetaEl) {
    transMetaEl.textContent = outcomeMeta(r);
    // 降级原因藏在 tooltip 里：平时不占地方，出问题时能查
    transMetaEl.title = r.note ?? "";
  }
}

function renderTransError(err: unknown): void {
  const msg = String(err);
  if (transEl) {
    transEl.classList.remove("placeholder");
    transEl.textContent = msg;
  }
  if (transMetaEl) {
    transMetaEl.textContent = "翻译失败";
    transMetaEl.title = msg;
  }
}

async function translate(): Promise<void> {
  if (!transEl || !transMetaEl) return;
  if (!lastText.trim()) {
    transEl.classList.add("placeholder");
    transEl.textContent = "暂无可翻译文本，请先识别";
    return;
  }

  transMetaEl.textContent = "翻译中…";
  transMetaEl.title = "";
  if (runBtn) runBtn.disabled = true;

  try {
    const r = await invoke<Outcome>("translate", { text: lastText });
    renderOutcome(r);
  } catch (err) {
    renderTransError(err);
  } finally {
    if (runBtn) runBtn.disabled = false;
  }
}

/* ---------------- 译文悬浮卡片 ---------------- */

/** 是否正处在悬浮形态（窗口已被后端缩成小卡片）。 */
let overlayOn = false;

/** 最近一次选区在屏幕上的矩形（物理像素）。悬浮卡片就贴着它摆。 */
let lastRect: { sx: number; sy: number; w: number; h: number; monitor: number } | null = null;

/** 卡片内容需要的真实高度（CSS 像素）。
 *
 * 不能直接用卡片的 scrollHeight：卡片带 overflow:hidden 且高度撑满窗口，
 * 它的 scrollHeight 永远等于窗口高度，量出来会是个死循环。
 * 所以改量滚动容器 `.ov-text` 的 scrollHeight（它装的是未被压缩的内容高度），
 * 再加上头部与内边距。 */
function overlayContentHeight(): number {
  if (!ovEl || !ovHeadEl || !ovTextEl) return 96;
  const cs = getComputedStyle(ovEl);
  const padV = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
  const hs = getComputedStyle(ovHeadEl);
  const headBox =
    ovHeadEl.offsetHeight + parseFloat(hs.marginTop) + parseFloat(hs.marginBottom);
  return Math.ceil(padV + headBox + ovTextEl.scrollHeight);
}

/** 让后端按内容高度调整卡片并重算位置。
 *
 * 换算成物理像素的活交给后端：它拿得到窗口的 scale_factor，
 * 前端自己乘 devicePixelRatio 会和它出现二义性（两者在多屏不同缩放下会分叉）。 */
function fitOverlay(): void {
  if (!overlayOn) return;
  const h = overlayContentHeight();
  void invoke("overlay_fit", { hLogical: h }).catch(() => {
    /* 已经退出悬浮形态了，忽略 */
  });
}

/** 进入悬浮形态并准备显示译文。锚点不存在时什么都不做。 */
async function showOverlay(): Promise<void> {
  if (!lastRect || !ovEl) return;

  // 先切 UI 再改窗口：反过来的话，窗口缩成卡片的那一帧里还挂着整个面板，
  // 会看到面板被压扁的一闪。
  overlayOn = true;
  document.body.classList.add("overlay-mode");
  ovEl.hidden = false;
  if (ovTextEl) {
    ovTextEl.textContent = "";
    ovTextEl.classList.remove("err");
  }
  if (ovMetaEl) ovMetaEl.textContent = "";

  try {
    await invoke("overlay_show", {
      screenX: lastRect.sx,
      screenY: lastRect.sy,
      width: lastRect.w,
      height: lastRect.h,
      monitor: lastRect.monitor,
    });
  } catch (err) {
    // 进不去悬浮不算致命：译文照样会落在面板里
    leaveOverlayUI();
    if (transMetaEl) transMetaEl.textContent = `悬浮显示失败：${String(err)}`;
    return;
  }
  requestAnimationFrame(fitOverlay);
}

/** 只切回面板的 UI，不动窗口（窗口由后端负责）。 */
function leaveOverlayUI(): void {
  overlayOn = false;
  document.body.classList.remove("overlay-mode");
  if (ovEl) ovEl.hidden = true;
}

/** 收起悬浮卡片，退回面板形态。译文仍留在面板里可以直接复制。 */
async function closeOverlay(): Promise<void> {
  if (!overlayOn) return;
  try {
    // 等后端把窗口还原成面板尺寸，再让面板内容显形——
    // 顺序反过来会看到一帧被拉扁的面板（和取消选区同一个道理）。
    await invoke("overlay_close");
  } catch {
    /* 窗口可能已销毁 */
  }
  leaveOverlayUI();
}

function setOverlayText(text: string, meta: string, isErr = false): void {
  if (ovTextEl) {
    ovTextEl.textContent = text;
    ovTextEl.classList.toggle("err", isErr);
  }
  if (ovMetaEl) ovMetaEl.textContent = meta;
  requestAnimationFrame(fitOverlay);
}

/** 框选识别完成后：按配置自动翻译，并把译文悬浮到原文框旁边。
 *
 * 顺序是"先出卡片再翻译"而不是"翻完再出卡片"：翻译 online 引擎要
 * 600–1200 ms，先把卡片摆到位置并显示"翻译中…"，用户至少知道
 * 东西出现在哪里，感知上比原地干等快得多。 */
async function autoTranslateAndOverlay(r: RegionResult): Promise<void> {
  const text = (r.ocr?.text ?? "").trim();
  if (!text) return;

  // 无论这次翻不翻，锚点都记下来：用户随后手点「翻译」时还能再用
  lastRect = {
    sx: r.screen_x,
    sy: r.screen_y,
    w: r.width,
    h: r.height,
    monitor: r.monitor,
  };

  if (!(mtCfg?.auto_translate ?? true)) return;

  await showOverlay();
  if (!overlayOn) return; // 悬浮没起来（窗口不在了），别继续做无用功
  setOverlayText("翻译中…", "");

  if (runBtn) runBtn.disabled = true;
  if (transMetaEl) transMetaEl.textContent = "翻译中…";
  try {
    const o = await invoke<Outcome>("translate", { text });
    renderOutcome(o);
    setOverlayText(o.text || "（无译文）", outcomeMeta(o));
  } catch (err) {
    renderTransError(err);
    setOverlayText(String(err).split("\n")[0], "翻译失败", true);
  } finally {
    if (runBtn) runBtn.disabled = false;
  }
}

/* ---------------- 区域框选 ---------------- */

/** 选区层的实际渲染矩形。
 *
 * 用 getBoundingClientRect 而不是 innerWidth：窗口正处在"面板尺寸 → 铺满整屏"
 * 的过渡中时，只有实际渲染尺寸才和用户看到的画面一致。 */
function layerRect(): DOMRect {
  return (
    selector?.getBoundingClientRect() ??
    new DOMRect(0, 0, window.innerWidth, window.innerHeight)
  );
}

/** CSS 像素矩形 → 帧像素矩形。两端的缩放比都从实际渲染尺寸反算，
 *  所以 DPI 缩放、多屏不同缩放比都不需要单独处理。 */
function toFrameRect(
  a: { x: number; y: number },
  b: { x: number; y: number },
): { x: number; y: number; w: number; h: number } {
  const rect = layerRect();
  const sx = frameW / (rect.width || 1);
  const sy = frameH / (rect.height || 1);
  return {
    x: Math.min(a.x, b.x) * sx,
    y: Math.min(a.y, b.y) * sy,
    w: Math.abs(a.x - b.x) * sx,
    h: Math.abs(a.y - b.y) * sy,
  };
}

function resetDrag(): void {
  dragFrom = null;
  if (selBox) selBox.hidden = true;
  if (selSize) selSize.hidden = true;
  if (selVLine) selVLine.hidden = true;
  if (selHLine) selHLine.hidden = true;
}

function hideOverlay(): void {
  if (selector) selector.hidden = true;
}

/** 等一帧绘制。用来确保"窗口尺寸已变"这件事被 WebView2 消化完再上遮罩层。 */
function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

async function beginRegion(): Promise<void> {
  if (selecting || regionBusy || !selector) return;

  const ocrWasDisabled = ocrBtn?.disabled ?? false;
  if (capMetaEl) capMetaEl.textContent = "准备选区…";

  // 先切形态再发请求：窗口一旦铺满会立即重排，
  // 先藏好面板就不会出现"被拉伸的一帧"。
  selecting = true;
  document.body.classList.add("selecting");
  // 遮罩层这时还不显示：它要在"窗口已经铺满、WebView2 已经在新尺寸上
  // 重排过一次"之后再出现。否则遮罩会按旧的 520×460 画一遍，
  // 窗口放大后这一帧不会自动重绘，屏幕上就会留下一个旧尺寸的黑洞。
  if (ocrBtn) ocrBtn.disabled = true;
  if (regionBtn) regionBtn.disabled = true;
  if (captureBtn) captureBtn.disabled = true;

  try {
    const r = await invoke<RegionBegin>("region_begin", { monitor: selectedMonitor() });
    frameW = r.width;
    frameH = r.height;
    resetDrag();
    if (selBg) selBg.src = `data:image/jpeg;base64,${r.jpeg_b64}`;

    // 两个 rAF：第一帧等布局与图片解码，第二帧等绘制。
    await nextFrame();
    await nextFrame();

    selector.hidden = false;
    if (capMetaEl) capMetaEl.textContent = `选区 ${r.width}×${r.height} · 采集 ${r.capture_ms} ms`;
  } catch (err) {
    leaveSelectMode();
    try {
      await invoke("region_cancel");
    } catch {
      /* 窗口可能已经不在了 */
    }
    if (capMetaEl) capMetaEl.textContent = `选区启动失败：${String(err)}`;
  } finally {
    if (ocrBtn) ocrBtn.disabled = ocrWasDisabled;
    if (regionBtn) regionBtn.disabled = false;
    if (captureBtn) captureBtn.disabled = false;
  }
}

function leaveSelectMode(): void {
  selecting = false;
  resetDrag();
  hideOverlay();
  document.body.classList.remove("selecting");
}

function onPointerDown(e: PointerEvent): void {
  if (!selecting) return;
  // 右键 = 取消，和多数截图工具一致
  if (e.button !== 0) {
    void cancelRegion();
    return;
  }
  const rect = layerRect();
  dragFrom = { x: e.clientX - rect.left, y: e.clientY - rect.top };
  // 指针可能已经被释放（合成事件、触摸取消），捕获失败不该让整个拖拽流程中断：
  // 没有捕获时只是离开窗口就收不到 move，仍在窗口内拖拽是正常的。
  try {
    selector?.setPointerCapture(e.pointerId);
  } catch {
    /* 没有可捕获的活动指针，忽略 */
  }
  e.preventDefault();
}

function onPointerMove(e: PointerEvent): void {
  if (!selecting) return;
  const rect = layerRect();
  const x = e.clientX - rect.left;
  const y = e.clientY - rect.top;

  if (!dragFrom) {
    // 未按下时只走十字准星，方便对准一行的上下沿
    if (selVLine) {
      selVLine.style.left = `${x}px`;
      selVLine.hidden = false;
    }
    if (selHLine) {
      selHLine.style.top = `${y}px`;
      selHLine.hidden = false;
    }
    return;
  }

  const left = Math.min(dragFrom.x, x);
  const top = Math.min(dragFrom.y, y);
  const w = Math.abs(x - dragFrom.x);
  const h = Math.abs(y - dragFrom.y);

  if (selBox) {
    selBox.style.left = `${left}px`;
    selBox.style.top = `${top}px`;
    selBox.style.width = `${w}px`;
    selBox.style.height = `${h}px`;
    selBox.hidden = false;
  }

  if (selSize) {
    const f = toFrameRect(dragFrom, { x, y });
    selSize.textContent = `${Math.round(f.w)} × ${Math.round(f.h)}`;
    selSize.hidden = false;
    // 默认贴选区右下角；快出屏时翻到选区上方
    const bx = Math.min(left + w + 8, rect.width - 96);
    const by = top + h + 8 > rect.height - 32 ? top - 26 : top + h + 8;
    selSize.style.left = `${Math.max(4, bx)}px`;
    selSize.style.top = `${Math.max(4, by)}px`;
  }
}

function onPointerUp(e: PointerEvent): void {
  if (!selecting || !dragFrom) return;
  const rect = layerRect();
  const x = e.clientX - rect.left;
  const y = e.clientY - rect.top;
  const from = dragFrom;
  dragFrom = null;

  if (Math.abs(x - from.x) < MIN_SELECT_PX || Math.abs(y - from.y) < MIN_SELECT_PX) {
    // 单击或手抖：不提交，留在选区形态等用户重新框
    resetDrag();
    return;
  }

  void finishRegion(toFrameRect(from, { x, y }));
}

async function finishRegion(crop: {
  x: number;
  y: number;
  w: number;
  h: number;
}): Promise<void> {
  // 遮罩先收，但 body 保持深色：后端一收到命令就把窗口还原成面板尺寸，
  // 这时若面板内容已经显示，会被压扁几帧。等结果回来再切。
  hideOverlay();
  resetDrag();
  selecting = false;
  regionBusy = true;
  if (regionBtn) regionBtn.disabled = true;
  if (captureBtn) captureBtn.disabled = true;

  if (capMetaEl) capMetaEl.textContent = "裁剪并识别…";
  if (ocrMetaEl) ocrMetaEl.textContent = "识别中…";

  try {
    const r = await invoke<RegionResult>("region_finish", {
      x: crop.x,
      y: crop.y,
      w: crop.w,
      h: crop.h,
      runOcr: true,
    });
    document.body.classList.remove("selecting");
    applyRegionResult(r);

    // 先放开按钮与 Esc：后面自动翻译要几百毫秒到一秒多，
    // 这期间若还按"选区进行中"算，用户按 Esc 会被吞掉，观感像卡住。
    regionBusy = false;
    if (regionBtn) regionBtn.disabled = false;
    if (captureBtn) captureBtn.disabled = false;

    await autoTranslateAndOverlay(r);
  } catch (err) {
    document.body.classList.remove("selecting");
    try {
      await invoke("region_cancel");
    } catch {
      /* 后端可能已经还原过了 */
    }
    if (ocrMetaEl) ocrMetaEl.textContent = "选区识别失败";
    if (ocrEl) {
      ocrEl.classList.remove("placeholder");
      ocrEl.textContent = String(err);
    }
  } finally {
    regionBusy = false;
    if (regionBtn) regionBtn.disabled = false;
    if (captureBtn) captureBtn.disabled = false;
  }
}

async function cancelRegion(): Promise<void> {
  if (!selecting) return;
  selecting = false;
  resetDrag();
  hideOverlay();
  try {
    // 等后端把窗口还原成面板尺寸，再让面板内容显形。
    // 顺序反过来会看到一帧被拉扁的面板。
    await invoke("region_cancel");
  } catch {
    /* 窗口可能已销毁 */
  }
  document.body.classList.remove("selecting");
  if (capMetaEl) capMetaEl.textContent = "已取消选区";
}

function applyRegionResult(r: RegionResult): void {
  showShot(r.jpeg_b64);
  if (boxesEl) boxesEl.innerHTML = "";

  if (capMetaEl) {
    capMetaEl.textContent = `选区 ${r.width}×${r.height} @ (${r.x},${r.y}) · 采集 ${r.capture_ms} · 裁剪 ${r.crop_ms} · 编码 ${r.encode_ms} ms`;
  }

  const o = r.ocr;
  if (!o) {
    if (ocrEl) {
      ocrEl.classList.add("placeholder");
      ocrEl.textContent = "已截取选区";
    }
    if (ocrMetaEl) ocrMetaEl.textContent = "OCR 未运行";
    return;
  }

  // 框坐标是相对裁剪图的，分母必须用裁剪后的尺寸
  renderBoxes(o.lines, r.width, r.height);
  lastText = o.text;
  if (ocrEl) {
    ocrEl.classList.toggle("placeholder", !lastText);
    ocrEl.textContent = lastText || "未识别到文字（选区偏小或对比度不足）";
  }
  if (ocrMetaEl) {
    ocrMetaEl.textContent = `${o.lines.length} 行 · 检测 ${o.det_ms} · 识别 ${o.rec_ms} · 合计 ${o.total_ms} ms`;
  }
  if (monitorSel && monitorSel.options.length === 0) {
    void loadMonitors(r.monitor);
  }
}

captureBtn?.addEventListener("click", () => {
  void capture();
});

ocrBtn?.addEventListener("click", () => {
  void runOcr();
});

runBtn?.addEventListener("click", () => {
  void translate();
});

/* ---------------- 翻译设置交互 ---------------- */

engineSel?.addEventListener("change", () => {
  // 快捷下拉只改引擎，其余配置保持不动
  void (async () => {
    const cfg = { ...(mtCfg ?? readCfg()), engine: engineSel.value };
    try {
      await invoke("translate_cfg_set", { cfg });
      mtCfg = cfg;
    } catch (err) {
      transMetaEl && (transMetaEl.textContent = `切换引擎失败：${String(err)}`);
    }
  })();
});

cfgEngineSel?.addEventListener("change", () => {
  showEnginePane(cfgEngineSel.value);
  if (cfgEngineSel.value === "offline" || cfgEngineSel.value === "auto") {
    void refreshMtStatus();
  }
});

cfgOpenBtn?.addEventListener("click", () => {
  void (async () => {
    try {
      engines = await invoke<EngineInfo[]>("translate_engines");
      const cfg = await invoke<MtCfg>("translate_cfg_get");
      applyCfg(cfg);
      await refreshMtStatus();
      if (cfgOut) cfgOut.textContent = "";
      if (cfgEl) cfgEl.hidden = false;
    } catch (err) {
      if (transMetaEl) transMetaEl.textContent = `打开设置失败：${String(err)}`;
    }
  })();
});

cfgCloseBtn?.addEventListener("click", () => {
  if (cfgEl) cfgEl.hidden = true;
});

// 点遮罩关闭（卡片内部的点击不冒泡到这里）
cfgEl?.addEventListener("click", (e) => {
  if (e.target === cfgEl) cfgEl.hidden = true;
});

cfgSaveBtn?.addEventListener("click", () => {
  void saveCfg(false);
});

cfgTestBtn?.addEventListener("click", () => {
  void testEngine();
});

cfgMtDl?.addEventListener("click", () => {
  void downloadModel();
});

cfgMtDir?.addEventListener("click", () => {
  void invoke("mt_open_dir").catch(() => {
    /* 打不开资源管理器不算错误，目录路径已经显示在状态里 */
  });
});

/* ---------------- 悬浮卡片交互 ---------------- */

ovBackBtn?.addEventListener("click", () => {
  void closeOverlay();
});

ovCopyBtn?.addEventListener("click", () => {
  const t = ovTextEl?.textContent ?? "";
  if (!t) return;
  void navigator.clipboard.writeText(t);
  if (ovCopyBtn) {
    ovCopyBtn.textContent = "已复制";
    window.setTimeout(() => {
      if (ovCopyBtn) ovCopyBtn.textContent = "复制";
    }, 1200);
  }
});

// 悬浮中按了热键：后端已经退出悬浮形态并广播出去，这里只需把 UI 切回面板。
// 不重新 invoke 一次 overlay_close——后端刚做过，再调一次会把还原后的几何又存一遍。
void listen("overlay-dismiss", () => {
  if (overlayOn) leaveOverlayUI();
});

// 下载进度：后端每 512 KB 发一次，这里只做进度条推进。
void listen<MtProgress>("mt-progress", (ev: { payload: MtProgress }) => {
  const p = ev.payload;
  if (!cfgDlFill || !cfgMtDl) return;
  const pct = p.overall_total > 0 ? (p.overall / p.overall_total) * 100 : 0;
  cfgDlFill.style.width = `${Math.min(100, pct).toFixed(1)}%`;
  if (p.file !== "done") {
    cfgMtDl.textContent = `下载中 ${pct.toFixed(0)}%（${bytesText(p.overall)} / ${bytesText(p.overall_total)}）`;
  }
});

regionBtn?.addEventListener("click", () => {
  void beginRegion();
});

selector?.addEventListener("pointerdown", onPointerDown);
selector?.addEventListener("pointermove", onPointerMove);
selector?.addEventListener("pointerup", onPointerUp);
selector?.addEventListener("pointercancel", () => {
  if (dragFrom) resetDrag();
});
selector?.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  void cancelRegion();
});

// Esc：选区形态下取消选区，常规形态下回收面板。
// window 被 destroy 而不是 hide，WebView2 子进程随之退出。
// 应用本体留在托盘，下次热键再按需重建。
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  // 悬浮形态下 Esc 只收卡片、不退面板：用户多半只是不想让它挡着，
  // 直接连面板一起销毁会把刚翻出来的译文一起丢掉。
  if (overlayOn) {
    e.preventDefault();
    void closeOverlay();
    return;
  }
  // 设置面板开着时，Esc 先关它——用户想退出的是那一层
  if (cfgEl && !cfgEl.hidden) {
    e.preventDefault();
    cfgEl.hidden = true;
    return;
  }
  if (selecting) {
    e.preventDefault();
    void cancelRegion();
    return;
  }
  if (regionBusy) {
    // 结果还在路上，别把面板关了
    e.preventDefault();
    return;
  }
  void invoke("close_panel");
});

// 依赖缺失时提前禁用按钮，避免用户点完才看到一长串报错。
void (async () => {
  try {
    await invoke("ocr_ready");
  } catch (err) {
    if (ocrBtn) {
      ocrBtn.disabled = true;
      ocrBtn.title = String(err);
    }
    if (ocrMetaEl) ocrMetaEl.textContent = `OCR 不可用：${String(err)}`;
  }
})();

// 翻译配置要在用户第一次点「翻译」之前就位，否则下拉框是空的。
void (async () => {
  try {
    engines = await invoke<EngineInfo[]>("translate_engines");
    const cfg = await invoke<MtCfg>("translate_cfg_get");
    applyCfg(cfg);
  } catch {
    /* 后端未就绪也不影响截图/OCR，静默失败 */
  }
})();

// 自动化钩子：窗口以 ?auto=ocr 打开时直接跑一次识别，便于截图取证。
if (new URLSearchParams(location.search).get("auto") === "ocr") {
  setTimeout(() => void runOcr(), 400);
}

// 自动化钩子：?auto=overlay 时用一段样例译文摆出悬浮卡片，便于截图取证。
// 真实锚点来自选区返回的屏幕坐标，这里用固定矩形代替——只为验证
// "卡片位置是否贴着框、高度是否被内容撑开、DPI 下文字是否正常"。
if (new URLSearchParams(location.search).get("auto") === "overlay") {
  setTimeout(() => {
    lastRect = { sx: 420, sy: 300, w: 460, h: 74, monitor: 0 };
    void showOverlay().then(() => {
      setOverlayText(
        "保存更改后再关闭？未保存的修改将会丢失，建议先按取消回到编辑器里确认一遍。",
        "自动 · 312 ms",
      );
    });
  }, 500);
}

// 刻意不在启动时枚举显示器：那会初始化 D3D 设备并常驻约 42 MB。
// 列表在首次截图成功后才加载（见 capture / runOcr）。
