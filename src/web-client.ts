interface ZipFileInfo {
  name: string;
  size: number;
  originalSize: number;
  compression: number;
}

interface FflateApi {
  zipSync(files: Record<string, Uint8Array>, options?: { level?: number }): Uint8Array;
  unzipSync(data: Uint8Array, options?: { filter?: (file: ZipFileInfo) => boolean }): Record<string, Uint8Array>;
}

declare global {
  interface Window { fflate: FflateApi; }
}

interface InputEntry {
  relativePath: string;
  file: File;
}

interface FolderSource {
  kind: "folder";
  name: string;
  entries: InputEntry[];
  bytes: number;
  detectedRoot?: string;
  valid: boolean;
  error?: string;
}

interface ZipSource {
  kind: "zip";
  name: string;
  file: File;
  bytes: number;
  valid: boolean;
  error?: string;
}

type SelectedSource = FolderSource | ZipSource;
type PhaseName = "package" | "upload" | "convert" | "download";
type PhaseState = "waiting" | "running" | "done" | "failed";

interface LegacyEntry {
  name: string;
  isFile: boolean;
  isDirectory: boolean;
}

interface LegacyFileEntry extends LegacyEntry {
  file(success: (file: File) => void, failure?: (error: DOMException) => void): void;
}

interface LegacyDirectoryReader {
  readEntries(success: (entries: LegacyEntry[]) => void, failure?: (error: DOMException) => void): void;
}

interface LegacyDirectoryEntry extends LegacyEntry {
  createReader(): LegacyDirectoryReader;
}

interface ConversionReport {
  success?: boolean;
  counts?: Record<string, unknown>;
  audit?: Record<string, unknown>;
  diagnostics?: Array<Record<string, unknown>>;
  web?: Record<string, unknown>;
  identity?: Record<string, unknown>;
}

const byId = <T extends HTMLElement>(id: string): T => {
  const element = document.getElementById(id);
  if (!element) throw new Error("Missing Web UI element: " + id);
  return element as T;
};

const dropZone = byId<HTMLDivElement>("dropZone");
const zipInput = byId<HTMLInputElement>("zipInput");
const folderInput = byId<HTMLInputElement>("folderInput");
const chooseZipButton = byId<HTMLButtonElement>("chooseZipButton");
const chooseFolderButton = byId<HTMLButtonElement>("chooseFolderButton");
const changeInputButton = byId<HTMLButtonElement>("changeInputButton");
const inputSummary = byId<HTMLDivElement>("inputSummary");
const inputError = byId<HTMLParagraphElement>("inputError");
const sourceIcon = byId<HTMLElement>("sourceIcon");
const sourceName = byId<HTMLElement>("sourceName");
const sourceDetail = byId<HTMLElement>("sourceDetail");
const sourceFiles = byId<HTMLElement>("sourceFiles");
const sourceSize = byId<HTMLElement>("sourceSize");
const detectionLine = byId<HTMLElement>("detectionLine");
const clientModeSelect = byId<HTMLSelectElement>("clientModeSelect");
const cmdPolicySelect = byId<HTMLSelectElement>("cmdPolicySelect");
const strictInput = byId<HTMLInputElement>("strictInput");
const auditInput = byId<HTMLInputElement>("auditInput");
const optionsFieldset = byId<HTMLFieldSetElement>("optionsFieldset");
const readyTitle = byId<HTMLElement>("readyTitle");
const readyDetail = byId<HTMLElement>("readyDetail");
const clearButton = byId<HTMLButtonElement>("clearButton");
const convertButton = byId<HTMLButtonElement>("convertButton");
const progressPanel = byId<HTMLElement>("progressPanel");
const resultPanel = byId<HTMLElement>("resultPanel");
const cancelButton = byId<HTMLButtonElement>("cancelButton");
const elapsedTime = byId<HTMLElement>("elapsedTime");
const liveStatus = byId<HTMLElement>("liveStatus");
const resultTitle = byId<HTMLElement>("result-title");
const resultMessage = byId<HTMLElement>("resultMessage");
const resultMark = byId<HTMLElement>("resultMark");
const resultEyebrow = byId<HTMLElement>("resultEyebrow");
const downloadButton = byId<HTMLButtonElement>("downloadButton");
const convertAnotherButton = byId<HTMLButtonElement>("convertAnotherButton");
const resultStats = byId<HTMLElement>("resultStats");
const auditBadge = byId<HTMLElement>("auditBadge");
const auditSummary = byId<HTMLElement>("auditSummary");
const diagnosticBadge = byId<HTMLElement>("diagnosticBadge");
const diagnosticSummary = byId<HTMLElement>("diagnosticSummary");
const diagnosticDetails = byId<HTMLDetailsElement>("diagnosticDetails");
const diagnosticCount = byId<HTMLElement>("diagnosticCount");
const diagnosticList = byId<HTMLElement>("diagnosticList");

const phaseElements: Record<PhaseName, { row: HTMLElement; text: HTMLElement; progress: HTMLProgressElement }> = {
  package: { row: byId("packageRow"), text: byId("packageText"), progress: byId("packageProgress") },
  upload: { row: byId("uploadRow"), text: byId("uploadText"), progress: byId("uploadProgress") },
  convert: { row: byId("convertRow"), text: byId("convertText"), progress: byId("convertProgress") },
  download: { row: byId("downloadRow"), text: byId("downloadText"), progress: byId("downloadProgress") },
};

let selectedSource: SelectedSource | undefined;
let activeRequest: XMLHttpRequest | undefined;
let resultBlobUrl: string | undefined;
let resultFilename = "craftengine-pack.zip";
let operationGeneration = 0;
let elapsedTimer: number | undefined;
let operationStarted = 0;
let maxUploadBytes = 256 * 1024 * 1024;
let maxExpandedBytes = 512 * 1024 * 1024;
let maxFileBytes = 128 * 1024 * 1024;
let maxSourceFiles = 25_000;
let busy = false;

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return (value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)) + " " + units[index];
}

function formatElapsed(milliseconds: number): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  return String(Math.floor(seconds / 60)).padStart(2, "0") + ":" + String(seconds % 60).padStart(2, "0");
}

function updateSteps(current: "select" | "configure" | "convert" | "download"): void {
  const order = ["select", "configure", "convert", "download"];
  const currentIndex = order.indexOf(current);
  document.querySelectorAll<HTMLElement>(".step").forEach((step) => {
    const index = order.indexOf(step.dataset.step ?? "");
    step.classList.toggle("is-active", index === currentIndex);
    step.classList.toggle("is-done", index >= 0 && index < currentIndex);
  });
}

function showInputError(message?: string): void {
  inputError.textContent = message ?? "";
  inputError.classList.toggle("is-hidden", !message);
}

function updateReadyState(): void {
  const sourceReady = selectedSource?.valid === true;
  convertButton.disabled = busy || !sourceReady;
  clearButton.disabled = busy || !selectedSource;
  if (!selectedSource) {
    readyTitle.textContent = "等待选择 Nexo 包";
    readyDetail.textContent = "文件只会发送到本机转换服务";
    updateSteps("select");
  } else if (!selectedSource.valid) {
    readyTitle.textContent = "来源需要处理";
    readyDetail.textContent = selectedSource.error ?? "未识别到 Nexo 根目录";
    updateSteps("select");
  } else {
    readyTitle.textContent = "可以开始转换";
    readyDetail.textContent = formatBytes(selectedSource.bytes) + " · 将从作者原包自动读取命名空间 · " + (selectedSource.kind === "folder" ? "文件夹会先在浏览器中打包" : "ZIP 将直接上传到本机");
    updateSteps("configure");
  }
}

function normalizeClientPath(path: string): string {
  const normalized = path.replaceAll("\\", "/").replace(/^\/+/, "");
  const segments = normalized.split("/");
  if (!normalized || segments.some((segment) => !segment || segment === "." || segment === "..")) {
    throw new Error("来源包含不安全路径：" + path);
  }
  return segments.join("/");
}

function detectFolderRoot(entries: InputEntry[]): { root?: string; error?: string; itemFiles: number; assets: number } {
  const roots = new Set<string>();
  let itemFiles = 0;
  let assets = 0;
  for (const entry of entries) {
    const lower = entry.relativePath.toLowerCase();
    const segments = entry.relativePath.split("/");
    for (let index = 0; index < segments.length - 1; index++) {
      const segment = segments[index]!.toLowerCase();
      if ((segment === "items" || segment === "item") && lower.endsWith(".yml")) {
        roots.add(segments.slice(0, index).join("/") || ".");
        itemFiles++;
        break;
      }
    }
  }
  if (roots.size === 0) return { error: "未找到包含 .yml 的 items/ 或 item/ 目录", itemFiles, assets };
  const rootList = Array.from(roots);
  const explicitNexo = rootList.filter((root) => root !== "." && root.split("/").at(-1)?.toLowerCase() === "nexo");
  if (explicitNexo.length > 1) return { error: "找到多个名为 Nexo 的根目录：" + explicitNexo.slice(0, 4).join(", "), itemFiles, assets };
  if (explicitNexo.length === 0 && rootList.length > 1) return { error: "找到多个候选根且没有明确的 Nexo 目录：" + rootList.slice(0, 4).join(", "), itemFiles, assets };
  const root = explicitNexo[0] ?? rootList[0]!;
  const prefix = root === "." ? "" : root + "/";
  assets = entries.filter((entry) => {
    const path = entry.relativePath.slice(prefix.length).toLowerCase();
    return entry.relativePath.startsWith(prefix) && (path.startsWith("pack/assets/") || path.startsWith("resourcepack/assets/") || path.startsWith("assets/"));
  }).length;
  return { root, itemFiles, assets };
}

function setFolderSource(entries: InputEntry[], name: string): void {
  try {
    const collisions = new Set<string>();
    const normalizedEntries = entries.map((entry) => {
      const path = normalizeClientPath(entry.relativePath);
      const key = path.normalize("NFC").toLowerCase();
      if (collisions.has(key)) throw new Error("来源包含重复或大小写冲突路径：" + path);
      collisions.add(key);
      return { relativePath: path, file: entry.file };
    });
    const bytes = normalizedEntries.reduce((sum, entry) => sum + entry.file.size, 0);
    const detection = detectFolderRoot(normalizedEntries);
    const oversizedEntry = normalizedEntries.find((entry) => entry.file.size > maxFileBytes);
    const limitError = normalizedEntries.length > maxSourceFiles
      ? "文件夹文件数量超过限制（" + maxSourceFiles + "）"
      : oversizedEntry
        ? "单个文件超过限制：" + oversizedEntry.relativePath + "（最大 " + formatBytes(maxFileBytes) + "）"
        : bytes > maxExpandedBytes
          ? "文件夹超过总展开大小限制（" + formatBytes(maxExpandedBytes) + "）"
          : bytes > maxUploadBytes
            ? "文件夹直接打包会超过 " + formatBytes(maxUploadBytes) + "；请先压缩成 ZIP 再选择"
            : undefined;
    selectedSource = {
      kind: "folder",
      name,
      entries: normalizedEntries,
      bytes,
      detectedRoot: detection.root,
      valid: !detection.error && !limitError,
      error: detection.error ?? limitError,
    };
    renderSource(detection.itemFiles, detection.assets);
  } catch (error) {
    selectedSource = undefined;
    showInputError(error instanceof Error ? error.message : String(error));
    updateReadyState();
  }
}

function setZipSource(file: File): void {
  const zipName = file.name.toLowerCase().endsWith(".zip");
  const sizeValid = file.size <= maxUploadBytes;
  selectedSource = {
    kind: "zip",
    name: file.name,
    file,
    bytes: file.size,
    valid: zipName && sizeValid,
    error: !zipName ? "请选择 .zip 文件" : !sizeValid ? "ZIP 超过上传限制（" + formatBytes(maxUploadBytes) + "）" : undefined,
  };
  renderSource(0, 0);
}

function renderSource(itemFiles: number, assets: number): void {
  if (!selectedSource) return;
  showInputError(selectedSource.error);
  dropZone.classList.add("is-hidden");
  inputSummary.classList.remove("is-hidden");
  changeInputButton.classList.remove("is-hidden");
  sourceIcon.textContent = selectedSource.kind === "folder" ? "DIR" : "ZIP";
  sourceName.textContent = selectedSource.name;
  sourceFiles.textContent = selectedSource.kind === "folder" ? String(selectedSource.entries.length) : "1";
  sourceSize.textContent = formatBytes(selectedSource.bytes);
  if (selectedSource.kind === "folder") {
    sourceDetail.textContent = "文件夹 · 浏览器打包后在本机转换";
    detectionLine.textContent = selectedSource.detectedRoot ? "已识别 Nexo 根目录：" + selectedSource.detectedRoot + " · " + itemFiles + " 个物品配置 · " + assets + " 个资源文件" : selectedSource.error ?? "等待识别";
  } else {
    sourceDetail.textContent = "ZIP · 由本机服务安全解压";
    detectionLine.textContent = "上传后自动识别任意层级中的 Nexo 根目录";
  }
  updateReadyState();
}

function clearSource(clearResult = true): void {
  operationGeneration++;
  activeRequest?.abort();
  activeRequest = undefined;
  selectedSource = undefined;
  zipInput.value = "";
  folderInput.value = "";
  dropZone.classList.remove("is-hidden", "is-dragging");
  inputSummary.classList.add("is-hidden");
  changeInputButton.classList.add("is-hidden");
  showInputError();
  progressPanel.classList.add("is-hidden");
  stopElapsedTimer();
  setBusy(false);
  if (clearResult) clearResultPanel();
  updateReadyState();
}

function clearResultPanel(): void {
  resultPanel.classList.add("is-hidden");
  resultPanel.classList.remove("is-partial", "is-failed");
  if (resultBlobUrl) URL.revokeObjectURL(resultBlobUrl);
  resultBlobUrl = undefined;
  resultStats.replaceChildren();
  auditSummary.replaceChildren();
  diagnosticSummary.replaceChildren();
  diagnosticList.replaceChildren();
}

function legacyFile(entry: LegacyFileEntry): Promise<File> {
  return new Promise((resolveFile, rejectFile) => entry.file(resolveFile, rejectFile));
}

function legacyReadBatch(reader: LegacyDirectoryReader): Promise<LegacyEntry[]> {
  return new Promise((resolveEntries, rejectEntries) => reader.readEntries(resolveEntries, rejectEntries));
}

async function traverseLegacyEntry(entry: LegacyEntry, parent: string, output: InputEntry[]): Promise<void> {
  const path = parent ? parent + "/" + entry.name : entry.name;
  if (entry.isFile) {
    output.push({ relativePath: path, file: await legacyFile(entry as LegacyFileEntry) });
    return;
  }
  if (entry.isDirectory) {
    const reader = (entry as LegacyDirectoryEntry).createReader();
    while (true) {
      const batch = await legacyReadBatch(reader);
      if (batch.length === 0) break;
      for (const child of batch) await traverseLegacyEntry(child, path, output);
    }
  }
}

async function handleDrop(dataTransfer: DataTransfer): Promise<void> {
  const ordinaryFiles = Array.from(dataTransfer.files);
  if (ordinaryFiles.length === 1 && ordinaryFiles[0]!.name.toLowerCase().endsWith(".zip")) {
    setZipSource(ordinaryFiles[0]!);
    return;
  }
  const entries: InputEntry[] = [];
  const transferItems = Array.from(dataTransfer.items);
  for (const item of transferItems) {
    const legacy = (item as DataTransferItem & { webkitGetAsEntry?: () => LegacyEntry | null }).webkitGetAsEntry?.();
    if (legacy) await traverseLegacyEntry(legacy, "", entries);
  }
  if (entries.length === 0) {
    for (const file of ordinaryFiles) entries.push({ relativePath: file.name, file });
  }
  if (entries.length === 0) throw new Error("没有从拖放内容中读取到文件，请使用“选择文件夹”按钮");
  const firstRoot = entries[0]!.relativePath.split("/")[0] ?? "Nexo";
  setFolderSource(entries, firstRoot);
}

function setPhase(name: PhaseName, state: PhaseState, text: string, percent?: number): void {
  const phase = phaseElements[name];
  phase.row.classList.toggle("is-running", state === "running");
  phase.row.classList.toggle("is-done", state === "done");
  phase.row.classList.toggle("is-failed", state === "failed");
  phase.text.textContent = text;
  if (percent === undefined && state === "running") phase.progress.removeAttribute("value");
  else {
    phase.progress.value = Math.max(0, Math.min(100, percent ?? (state === "done" ? 100 : 0)));
    phase.progress.setAttribute("value", String(phase.progress.value));
  }
}

function resetProgress(): void {
  setPhase("package", "waiting", "等待", 0);
  setPhase("upload", "waiting", "等待", 0);
  setPhase("convert", "waiting", "等待", 0);
  setPhase("download", "waiting", "等待", 0);
}

function setBusy(value: boolean): void {
  busy = value;
  optionsFieldset.disabled = value;
  chooseFolderButton.disabled = value;
  chooseZipButton.disabled = value;
  changeInputButton.disabled = value;
  cancelButton.disabled = !value;
  if (value) {
    convertButton.disabled = true;
    clearButton.disabled = true;
  } else {
    updateReadyState();
    if (!resultPanel.classList.contains("is-hidden")) updateSteps("download");
  }
}

function startElapsedTimer(): void {
  stopElapsedTimer();
  operationStarted = performance.now();
  elapsedTime.textContent = "00:00";
  elapsedTimer = window.setInterval(() => { elapsedTime.textContent = formatElapsed(performance.now() - operationStarted); }, 500);
}

function stopElapsedTimer(): void {
  if (elapsedTimer !== undefined) window.clearInterval(elapsedTimer);
  elapsedTimer = undefined;
}

function nextFrame(): Promise<void> {
  return new Promise((resolveFrame) => requestAnimationFrame(() => resolveFrame()));
}

async function packageFolder(source: FolderSource, generation: number): Promise<Blob> {
  const files: Record<string, Uint8Array> = {};
  let completedBytes = 0;
  setPhase("package", "running", "读取 0 / " + source.entries.length + " 个文件", 0);
  liveStatus.textContent = "正在浏览器中打包文件夹…";
  for (let index = 0; index < source.entries.length; index++) {
    if (generation !== operationGeneration) throw new DOMException("Conversion cancelled", "AbortError");
    const entry = source.entries[index]!;
    files[entry.relativePath] = new Uint8Array(await entry.file.arrayBuffer());
    completedBytes += entry.file.size;
    const percent = source.bytes > 0 ? completedBytes / source.bytes * 92 : (index + 1) / source.entries.length * 92;
    setPhase("package", "running", "读取 " + (index + 1) + " / " + source.entries.length + " · " + formatBytes(completedBytes), percent);
  }
  setPhase("package", "running", "正在生成 ZIP…", undefined);
  await nextFrame();
  const zipped = window.fflate.zipSync(files, { level: 0 });
  if (zipped.byteLength > maxUploadBytes) throw new Error("打包后的 ZIP 超过上传限制（" + formatBytes(maxUploadBytes) + "）");
  setPhase("package", "done", "已打包 " + formatBytes(zipped.byteLength), 100);
  const zipBuffer = new ArrayBuffer(zipped.byteLength);
  new Uint8Array(zipBuffer).set(zipped);
  return new Blob([zipBuffer], { type: "application/zip" });
}

function apiUrl(): string {
  const page = new URL(window.location.href);
  const token = page.searchParams.get("token");
  if (!token) throw new Error("本地会话令牌缺失，请刷新页面");
  const query = new URLSearchParams({
    token,
    clientMode: clientModeSelect.value,
    cmdPolicy: cmdPolicySelect.value,
    strict: String(strictInput.checked),
    audit: String(auditInput.checked),
  });
  return "/api/convert?" + query.toString();
}

function uploadZip(blob: Blob, generation: number): Promise<Blob> {
  return new Promise((resolveUpload, rejectUpload) => {
    const xhr = new XMLHttpRequest();
    activeRequest = xhr;
    xhr.open("POST", apiUrl());
    xhr.responseType = "blob";
    xhr.setRequestHeader("Content-Type", "application/zip");
    xhr.upload.addEventListener("progress", (event) => {
      if (generation !== operationGeneration) return;
      const percent = event.lengthComputable && event.total > 0 ? event.loaded / event.total * 100 : undefined;
      setPhase("upload", "running", formatBytes(event.loaded) + (event.total ? " / " + formatBytes(event.total) : ""), percent);
      liveStatus.textContent = "正在上传到 127.0.0.1…";
    });
    xhr.upload.addEventListener("load", () => {
      if (generation !== operationGeneration) return;
      setPhase("upload", "done", "已发送 " + formatBytes(blob.size), 100);
      setPhase("convert", "running", "服务器正在转换和审计…", undefined);
      liveStatus.textContent = "正在按 Nexo、CraftEngine 和 Minecraft 语义转换…";
    });
    let receiving = false;
    xhr.addEventListener("progress", (event) => {
      if (generation !== operationGeneration) return;
      if (!receiving) {
        receiving = true;
        setPhase("convert", "done", "转换已完成", 100);
      }
      const percent = event.lengthComputable && event.total > 0 ? event.loaded / event.total * 100 : undefined;
      setPhase("download", "running", "已接收 " + formatBytes(event.loaded), percent);
      liveStatus.textContent = "正在接收 CraftEngine ZIP…";
    });
    xhr.addEventListener("load", () => {
      if (generation !== operationGeneration) return;
      activeRequest = undefined;
      const response = xhr.response as Blob;
      if (xhr.status >= 200 && xhr.status < 300 && (response.type === "application/zip" || xhr.getResponseHeader("Content-Type")?.startsWith("application/zip"))) {
        setPhase("convert", "done", "转换已完成", 100);
        setPhase("download", "done", "已接收 " + formatBytes(response.size), 100);
        resolveUpload(response);
        return;
      }
      void response.text().then((text) => {
        try {
          const problem = JSON.parse(text) as { code?: string; detail?: string };
          rejectUpload(new Error((problem.code ? problem.code + "：" : "") + (problem.detail ?? "本地转换请求失败")));
        } catch {
          rejectUpload(new Error("本地转换请求失败（HTTP " + xhr.status + "）"));
        }
      });
    });
    xhr.addEventListener("error", () => rejectUpload(new Error("无法连接本机转换服务，请确认终端中的 Web 服务仍在运行")));
    xhr.addEventListener("abort", () => rejectUpload(new DOMException("Conversion cancelled", "AbortError")));
    setPhase("upload", "running", "准备上传…", 0);
    xhr.send(blob);
  });
}

function decodeJson(bytes?: Uint8Array): Record<string, unknown> | undefined {
  if (!bytes) return undefined;
  try { return JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>; }
  catch { return undefined; }
}

async function inspectOutput(blob: Blob): Promise<{ report: ConversionReport; metadata?: Record<string, unknown> }> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  const selected = window.fflate.unzipSync(bytes, {
    filter: (file) => /(^|\/)conversion-(?:report|response)\.json$/.test(file.name),
  });
  const reportEntry = Object.entries(selected).find(([name]) => /(^|\/)conversion-report\.json$/.test(name));
  const metadataEntry = Object.entries(selected).find(([name]) => /(^|\/)conversion-response\.json$/.test(name));
  const report = decodeJson(reportEntry?.[1]) as ConversionReport | undefined;
  if (!report) throw new Error("输出 ZIP 缺少 conversion-report.json");
  return { report, metadata: decodeJson(metadataEntry?.[1]) };
}

function numberValue(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function objectValue(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function appendStat(value: number, label: string): void {
  const card = document.createElement("div");
  card.className = "stat";
  const number = document.createElement("b");
  number.textContent = String(value);
  const text = document.createElement("span");
  text.textContent = label;
  card.append(number, text);
  resultStats.append(card);
}

function appendAudit(label: string, value: number): void {
  const term = document.createElement("dt");
  term.textContent = label;
  const description = document.createElement("dd");
  description.textContent = String(value);
  auditSummary.append(term, description);
}

function appendDiagnosticMetric(value: number, label: string, className: string): void {
  const card = document.createElement("div");
  card.className = "diag-metric " + className;
  const number = document.createElement("b");
  number.textContent = String(value);
  const text = document.createElement("span");
  text.textContent = label;
  card.append(number, text);
  diagnosticSummary.append(card);
}

function diagnosticLocation(diagnostic: Record<string, unknown>): string {
  return [diagnostic.source, diagnostic.item, diagnostic.field].filter((value) => typeof value === "string" && value.length > 0).join(" / ");
}

interface DiagnosticGroup {
  code: string;
  severity: string;
  lossy: boolean;
  message: string;
  diagnostics: Array<Record<string, unknown>>;
}

const diagnosticGuidance: Record<string, { message: string; note?: string }> = {
  FURNITURE_WALL_VERTICAL_OFFSET_DYNAMIC: {
    message: "该未受限 FIXED 墙面配置仍有依赖下方支撑的垂直位移。",
    note: "limited wall 已由 Material.isSolid 原生 profiles 自动处理；未受限模式同时受 Nexo 玩家 yaw 与多放置面默认值影响，转换器不会用错误的静态偏移掩盖它。",
  },
  FURNITURE_TOGGLED_LIGHT_MODEL_UNSUPPORTED: {
    message: "灯光开关已转换，但 Nexo 为关闭状态指定的替代显示模型需要单独建立 CraftEngine 物品。",
  },
};

function groupDiagnostics(diagnostics: Array<Record<string, unknown>>): DiagnosticGroup[] {
  const groups = new Map<string, DiagnosticGroup>();
  for (const diagnostic of diagnostics) {
    const code = typeof diagnostic.code === "string" ? diagnostic.code : "DIAGNOSTIC";
    const severity = typeof diagnostic.severity === "string" ? diagnostic.severity : "info";
    const lossy = diagnostic.lossy === true;
    const message = typeof diagnostic.message === "string" ? diagnostic.message : "";
    const key = [severity, lossy ? "lossy" : "exact", code, message].join("\u0000");
    const group = groups.get(key);
    if (group) group.diagnostics.push(diagnostic);
    else groups.set(key, { code, severity, lossy, message, diagnostics: [diagnostic] });
  }
  const priority: Record<string, number> = { error: 0, warning: 1, info: 2 };
  return Array.from(groups.values()).sort((left, right) => (priority[left.severity] ?? 3) - (priority[right.severity] ?? 3));
}

function severityLabel(severity: string): string {
  if (severity === "error") return "错误";
  if (severity === "warning") return "警告";
  return "信息";
}

function renderDiagnostics(groups: DiagnosticGroup[]): void {
  diagnosticList.replaceChildren();
  const visible = groups.slice(0, 100);
  for (const group of visible) {
    const item = document.createElement("div");
    item.className = "diagnostic-item";
    const level = document.createElement("span");
    level.className = "diag-level " + group.severity;
    level.textContent = severityLabel(group.severity) + (group.lossy ? " · 有损" : "") + (group.diagnostics.length > 1 ? " ×" + group.diagnostics.length : "");
    const body = document.createElement("div");
    body.className = "diag-body";
    const code = document.createElement("strong");
    code.textContent = group.code;
    const guidance = diagnosticGuidance[group.code];
    const message = document.createElement("p");
    message.textContent = guidance?.message ?? group.message;
    body.append(code, message);
    if (guidance?.note) {
      const note = document.createElement("p");
      note.className = "diag-guidance";
      note.textContent = guidance.note;
      body.append(note);
    }
    const locations = Array.from(new Set(group.diagnostics.map(diagnosticLocation).filter(Boolean)));
    if (locations.length === 1) {
      const path = document.createElement("small");
      path.textContent = locations[0]!;
      body.append(path);
    } else if (locations.length > 1) {
      const occurrences = document.createElement("details");
      occurrences.className = "diag-occurrences";
      const summary = document.createElement("summary");
      summary.textContent = "受影响 " + locations.length + " 项（展开查看）";
      const list = document.createElement("ul");
      for (const location of locations.slice(0, 100)) {
        const row = document.createElement("li");
        row.textContent = location;
        list.append(row);
      }
      if (locations.length > 100) {
        const row = document.createElement("li");
        row.textContent = "其余 " + (locations.length - 100) + " 项请查看 conversion-report.json";
        list.append(row);
      }
      occurrences.append(summary, list);
      body.append(occurrences);
    }
    item.append(level, body);
    diagnosticList.append(item);
  }
  if (groups.length > visible.length) {
    const omitted = document.createElement("div");
    omitted.className = "diagnostic-item";
    omitted.textContent = "还有 " + (groups.length - visible.length) + " 类诊断，请查看 ZIP 内的 conversion-report.json。";
    diagnosticList.append(omitted);
  }
}

function renderResult(blob: Blob, report: ConversionReport): void {
  clearResultPanel();
  const counts = objectValue(report.counts);
  const diagnosticsCounts = objectValue(counts.diagnostics);
  const diagnostics = Array.isArray(report.diagnostics) ? report.diagnostics : [];
  const diagnosticGroups = groupDiagnostics(diagnostics);
  const errors = numberValue(diagnosticsCounts.error);
  const warnings = numberValue(diagnosticsCounts.warning);
  const lossy = numberValue(diagnosticsCounts.lossy);
  const success = report.success === true;
  const identity = objectValue(report.identity);
  const detectedNamespace = typeof identity.targetItemNamespace === "string" ? identity.targetItemNamespace : "未知";

  resultPanel.classList.remove("is-hidden");
  resultPanel.classList.toggle("is-partial", !success && blob.size > 0);
  resultEyebrow.textContent = success ? "04 / COMPLETE" : "04 / REVIEW REQUIRED";
  resultTitle.textContent = success ? "转换完成" : "已生成结果，但需要检查";
  resultMessage.textContent = success
    ? diagnosticGroups.length === 0
      ? "已按作者命名空间 " + detectedNamespace + " 生成 CraftEngine ZIP，内部为官方 resources/" + detectedNamespace + " 结构；直接解压到 plugins/CraftEngine/。"
      : "转换成功；" + diagnostics.length + " 条诊断已合并为 " + diagnosticGroups.length + " 类。ZIP 已按官方 resources/" + detectedNamespace + " 结构打包，直接解压到 plugins/CraftEngine/。"
    : "报告中存在错误或严格模式不接受的有损项；仍可下载结果进行修正。";
  resultMark.textContent = success ? "✓" : "!";
  resultBlobUrl = URL.createObjectURL(blob);
  const sourceStem = selectedSource?.name.replace(/\.zip$/i, "").replace(/[^\p{L}\p{N}._-]+/gu, "_").replace(/^_+|_+$/g, "") || "nexo-pack";
  resultFilename = "craftengine-" + sourceStem + ".zip";
  downloadButton.classList.remove("is-hidden");

  appendStat(numberValue(counts.items), "物品");
  appendStat(numberValue(counts.furniture), "家具");
  appendStat(numberValue(counts.blocks), "方块");
  appendStat(numberValue(counts.recipes), "配方");
  appendStat(numberValue(counts.resources), "资源文件");
  appendStat(diagnostics.length, "诊断");

  const audit = objectValue(report.audit);
  auditSummary.replaceChildren();
  appendAudit("已解析模型", numberValue(audit.resolvedModels));
  appendAudit("缺失模型", numberValue(audit.missingModels));
  appendAudit("已解析纹理", numberValue(audit.resolvedTextures));
  appendAudit("缺失纹理", numberValue(audit.missingTextures));
  appendAudit("缺失 Blueprint", numberValue(audit.missingBlueprints));
  const auditFailures = numberValue(audit.missingModels) + numberValue(audit.missingTextures) + numberValue(audit.missingBlueprints);
  auditBadge.textContent = Object.keys(audit).length === 0 ? "未运行" : auditFailures === 0 ? "通过" : auditFailures + " 缺失";

  diagnosticSummary.replaceChildren();
  appendDiagnosticMetric(errors, "错误", "error");
  appendDiagnosticMetric(warnings, "警告", "warning");
  appendDiagnosticMetric(lossy, "有损", "lossy");
  diagnosticBadge.textContent = diagnosticGroups.length === diagnostics.length ? String(diagnostics.length) : diagnosticGroups.length + " 类";
  diagnosticCount.textContent = diagnosticGroups.length === diagnostics.length ? String(diagnostics.length) : diagnosticGroups.length + " 类 / " + diagnostics.length + " 项";
  diagnosticDetails.classList.toggle("is-hidden", diagnostics.length === 0);
  renderDiagnostics(diagnosticGroups);
  updateSteps("download");
  window.setTimeout(() => resultTitle.focus(), 0);
  resultPanel.scrollIntoView({ behavior: "smooth", block: "start" });
}

function renderFatalError(error: unknown): void {
  resultPanel.classList.remove("is-hidden", "is-partial");
  resultPanel.classList.add("is-failed");
  resultTitle.textContent = "转换失败";
  resultMessage.textContent = error instanceof Error ? error.message : String(error);
  resultEyebrow.textContent = "ERROR";
  resultMark.textContent = "×";
  downloadButton.classList.add("is-hidden");
  resultStats.replaceChildren();
  auditSummary.replaceChildren();
  diagnosticSummary.replaceChildren();
  diagnosticDetails.classList.add("is-hidden");
  window.setTimeout(() => resultTitle.focus(), 0);
  resultPanel.scrollIntoView({ behavior: "smooth", block: "start" });
}

async function runConversion(): Promise<void> {
  if (!selectedSource?.valid || busy) return;
  const generation = ++operationGeneration;
  clearResultPanel();
  resetProgress();
  progressPanel.classList.remove("is-hidden");
  updateSteps("convert");
  setBusy(true);
  startElapsedTimer();
  liveStatus.textContent = "正在准备来源…";
  progressPanel.scrollIntoView({ behavior: "smooth", block: "start" });
  try {
    let upload: Blob;
    if (selectedSource.kind === "folder") upload = await packageFolder(selectedSource, generation);
    else {
      upload = selectedSource.file;
      setPhase("package", "done", "已使用现有 ZIP · " + formatBytes(upload.size), 100);
    }
    if (generation !== operationGeneration) throw new DOMException("Conversion cancelled", "AbortError");
    const output = await uploadZip(upload, generation);
    const inspected = await inspectOutput(output);
    if (generation !== operationGeneration) return;
    liveStatus.textContent = "转换结果已经准备好";
    renderResult(output, inspected.report);
  } catch (error) {
    if (generation !== operationGeneration || (error instanceof DOMException && error.name === "AbortError")) {
      liveStatus.textContent = "转换已取消";
      progressPanel.classList.add("is-hidden");
      updateSteps(selectedSource ? "configure" : "select");
    } else {
      setPhase("convert", "failed", "转换失败", 0);
      liveStatus.textContent = "转换失败";
      renderFatalError(error);
    }
  } finally {
    if (generation === operationGeneration) {
      activeRequest = undefined;
      stopElapsedTimer();
      setBusy(false);
    }
  }
}

function triggerDownload(): void {
  if (!resultBlobUrl) return;
  const anchor = document.createElement("a");
  anchor.href = resultBlobUrl;
  anchor.download = resultFilename;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
}

chooseZipButton.addEventListener("click", (event) => { event.stopPropagation(); zipInput.click(); });
chooseFolderButton.addEventListener("click", (event) => { event.stopPropagation(); folderInput.click(); });
changeInputButton.addEventListener("click", () => clearSource(false));
clearButton.addEventListener("click", () => clearSource());
convertAnotherButton.addEventListener("click", () => clearSource());
convertButton.addEventListener("click", () => { void runConversion(); });
downloadButton.addEventListener("click", triggerDownload);
cancelButton.addEventListener("click", () => {
  operationGeneration++;
  activeRequest?.abort();
  activeRequest = undefined;
  stopElapsedTimer();
  setBusy(false);
  progressPanel.classList.add("is-hidden");
  liveStatus.textContent = "转换已取消";
  updateSteps(selectedSource ? "configure" : "select");
});
zipInput.addEventListener("change", () => {
  const file = zipInput.files?.[0];
  if (file) setZipSource(file);
});
folderInput.addEventListener("change", () => {
  const files = Array.from(folderInput.files ?? []);
  if (files.length === 0) return;
  const entries = files.map((file) => ({ relativePath: file.webkitRelativePath || file.name, file }));
  const rootName = entries[0]!.relativePath.split("/")[0] ?? "Nexo";
  setFolderSource(entries, rootName);
});

dropZone.addEventListener("keydown", (event) => {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    zipInput.click();
  }
});
for (const name of ["dragenter", "dragover"]) dropZone.addEventListener(name, (event) => {
  event.preventDefault();
  dropZone.classList.add("is-dragging");
});
for (const name of ["dragleave", "drop"]) dropZone.addEventListener(name, (event) => {
  event.preventDefault();
  dropZone.classList.remove("is-dragging");
});
dropZone.addEventListener("drop", (event) => {
  if (!event.dataTransfer) return;
  void handleDrop(event.dataTransfer).catch((error) => showInputError(error instanceof Error ? error.message : String(error)));
});

window.addEventListener("beforeunload", () => {
  activeRequest?.abort();
  if (resultBlobUrl) URL.revokeObjectURL(resultBlobUrl);
});

async function loadHealth(): Promise<void> {
  try {
    const response = await fetch("/api/health", { cache: "no-store" });
    if (!response.ok) return;
    const health = await response.json() as { limits?: { maxUploadBytes?: number; maxUncompressedBytes?: number; maxFileBytes?: number; maxFiles?: number } };
    if (typeof health.limits?.maxUploadBytes === "number") maxUploadBytes = health.limits.maxUploadBytes;
    if (typeof health.limits?.maxUncompressedBytes === "number") maxExpandedBytes = health.limits.maxUncompressedBytes;
    if (typeof health.limits?.maxFileBytes === "number") maxFileBytes = health.limits.maxFileBytes;
    if (typeof health.limits?.maxFiles === "number") maxSourceFiles = health.limits.maxFiles;
  } catch {
    showInputError("无法读取本机服务状态；转换时会再次检查连接");
  }
}

resetProgress();
updateReadyState();
void loadHealth();

export {};
