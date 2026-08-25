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
const actionBar = byId<HTMLElement>("actionBar");
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
const copyDiagnosticsButton = byId<HTMLButtonElement>("copyDiagnosticsButton");
const diagnosticSearch = byId<HTMLInputElement>("diagnosticSearch");
const toastContainer = byId<HTMLDivElement>("toastContainer");

const phaseElements: Record<PhaseName, { row: HTMLElement; text: HTMLElement; progress: HTMLProgressElement }> = {
  package: { row: byId("packageRow"), text: byId("packageText"), progress: byId("packageProgress") },
  upload: { row: byId("uploadRow"), text: byId("uploadText"), progress: byId("uploadProgress") },
  convert: { row: byId("convertRow"), text: byId("convertText"), progress: byId("convertProgress") },
  download: { row: byId("downloadRow"), text: byId("downloadText"), progress: byId("downloadProgress") },
};

let selectedSource: SelectedSource | undefined;
let activeRequest: XMLHttpRequest | undefined;
let resultBlobUrl: string | undefined;
let resultFilename = "craftengine-output.zip";
let operationStarted = 0;
let elapsedTimer: number | undefined;
let operationGeneration = 0;
let maxUploadBytes = 256 * 1024 * 1024;
let maxExpandedBytes = 512 * 1024 * 1024;
let maxFileBytes = 128 * 1024 * 1024;
let maxSourceFiles = 25_000;
let busy = false;
let currentDiagnosticGroups: DiagnosticGroup[] = [];
let currentDiagnosticFilter = "all";
let currentDiagnosticSearchQuery = "";
let latestConversionReport: ConversionReport | undefined;

function showToast(message: string, isSuccess = true): void {
  const toast = document.createElement("div");
  toast.className = "toast" + (isSuccess ? " toast-success" : "");
  toast.textContent = message;
  toastContainer.append(toast);
  window.setTimeout(() => {
    toast.style.opacity = "0";
    toast.style.transform = "translateY(8px) scale(0.96)";
    toast.style.transition = "all 0.25s ease";
    window.setTimeout(() => toast.remove(), 250);
  }, 3200);
}

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
  actionBar.classList.toggle("is-ready", sourceReady && !busy);

  if (!selectedSource) {
    readyTitle.textContent = "等待选择 Nexo 包";
    readyDetail.textContent = "所有处理均在当前浏览器与本机后台执行，无远程上传";
    updateSteps("select");
  } else if (!selectedSource.valid) {
    readyTitle.textContent = "来源需要处理";
    readyDetail.textContent = selectedSource.error ?? "未识别到有效的 Nexo 根目录";
    updateSteps("select");
  } else {
    readyTitle.textContent = "准备就绪，可以开始转换";
    readyDetail.textContent = formatBytes(selectedSource.bytes) + " · 自动解析作者命名空间 · " + (selectedSource.kind === "folder" ? "文件夹将在浏览器打包后传输" : "ZIP 压缩包将直接传输解析");
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
    showToast("已成功载入文件夹：" + name);
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
    error: !zipName ? "请选择 .zip 格式的压缩文件" : !sizeValid ? "ZIP 超过上传限制（" + formatBytes(maxUploadBytes) + "）" : undefined,
  };
  renderSource(0, 0);
  if (selectedSource.valid) {
    showToast("已载入 ZIP 资源包：" + file.name);
  }
}

function renderSource(itemFiles: number, assets: number): void {
  if (!selectedSource) return;
  showInputError(selectedSource.error);
  dropZone.classList.add("is-hidden");
  inputSummary.classList.remove("is-hidden");
  changeInputButton.classList.remove("is-hidden");
  sourceIcon.textContent = selectedSource.kind === "folder" ? "DIR" : "ZIP";
  sourceName.textContent = selectedSource.name;
  sourceFiles.textContent = selectedSource.kind === "folder" ? String(selectedSource.entries.length) : "1 个压缩包";
  sourceSize.textContent = formatBytes(selectedSource.bytes);
  if (selectedSource.kind === "folder") {
    sourceDetail.textContent = "文件夹来源 · 将在当前浏览器直接打包传输至转换引擎";
    detectionLine.textContent = selectedSource.detectedRoot
      ? "已定位 Nexo 根目录：[" + selectedSource.detectedRoot + "] · 发现 " + itemFiles + " 个物品配置 · " + assets + " 个模型材质文件"
      : selectedSource.error ?? "等待解析目录结构";
  } else {
    sourceDetail.textContent = "ZIP 压缩包 · 由本地服务执行安全解压与智能定位";
    detectionLine.textContent = "开始转换后将自动扫描任意层级寻找 Nexo 根目录与作者命名空间";
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
  currentDiagnosticGroups = [];
  latestConversionReport = undefined;
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
  if (entries.length === 0) throw new Error("未能从拖入内容中读取到文件，请使用“选择文件夹”或“选择 ZIP”");
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
  elapsedTimer = window.setInterval(() => {
    elapsedTime.textContent = formatElapsed(performance.now() - operationStarted);
  }, 250);
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
  liveStatus.textContent = "正在读取并打包文件夹…";
  for (let index = 0; index < source.entries.length; index++) {
    if (generation !== operationGeneration) throw new DOMException("Conversion cancelled", "AbortError");
    const entry = source.entries[index]!;
    files[entry.relativePath] = new Uint8Array(await entry.file.arrayBuffer());
    completedBytes += entry.file.size;
    const percent = source.bytes > 0 ? (completedBytes / source.bytes) * 92 : ((index + 1) / source.entries.length) * 92;
    setPhase("package", "running", "打包 " + (index + 1) + " / " + source.entries.length + " · " + formatBytes(completedBytes), percent);
  }
  setPhase("package", "running", "正在生成压缩包…", undefined);
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
  if (!token) throw new Error("本地会话令牌缺失，请刷新页面重新连接");
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
      const percent = event.lengthComputable && event.total > 0 ? (event.loaded / event.total) * 100 : undefined;
      setPhase("upload", "running", formatBytes(event.loaded) + (event.total ? " / " + formatBytes(event.total) : ""), percent);
      liveStatus.textContent = "正在传输数据至本地转换服务…";
    });
    xhr.upload.addEventListener("load", () => {
      if (generation !== operationGeneration) return;
      setPhase("upload", "done", "已发送 " + formatBytes(blob.size), 100);
      setPhase("convert", "running", "语义解析与资源图审计中…", undefined);
      liveStatus.textContent = "正在按 Nexo、CraftEngine 26.8 与 Minecraft 1.21.11 官方 Codec 转换…";
    });
    let receiving = false;
    xhr.addEventListener("progress", (event) => {
      if (generation !== operationGeneration) return;
      if (!receiving) {
        receiving = true;
        setPhase("convert", "done", "转换与审计已完成", 100);
      }
      const percent = event.lengthComputable && event.total > 0 ? (event.loaded / event.total) * 100 : undefined;
      setPhase("download", "running", "已组装 " + formatBytes(event.loaded), percent);
      liveStatus.textContent = "正在接收生成的 CraftEngine ZIP…";
    });
    xhr.addEventListener("load", () => {
      if (generation !== operationGeneration) return;
      activeRequest = undefined;
      const response = xhr.response as Blob;
      if (xhr.status >= 200 && xhr.status < 300 && (response.type === "application/zip" || xhr.getResponseHeader("Content-Type")?.startsWith("application/zip"))) {
        setPhase("convert", "done", "转换与审计已完成", 100);
        setPhase("download", "done", "已完成 · " + formatBytes(response.size), 100);
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
    xhr.addEventListener("error", () => rejectUpload(new Error("无法连接本机转换服务，请确认终端中的 Web 服务仍处于运行状态")));
    xhr.addEventListener("abort", () => rejectUpload(new DOMException("Conversion cancelled", "AbortError")));
    setPhase("upload", "running", "准备传输…", 0);
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
    note: "转换器采用简洁的单一 wall 基础变体，不再生成冗长 support profiles；依赖下方支撑的动态位移可根据需要手动微调。",
  },
  FURNITURE_TOGGLED_LIGHT_MODEL_UNSUPPORTED: {
    message: "灯光开关已转换，但 Nexo 为关闭状态指定的替代显示模型需要单独建立 CraftEngine 物品。",
  },
  COMPONENT_CODEC_MANUAL: {
    message: "该 Data Component 包含需要运行时注册表展开、外部 ItemStack 或无法静态判定的字段，已安全省略以防加载异常。",
    note: "16 类常见 Builder Components 已按官方 1.21.11 Codec 安全展开；仅特殊动态结构保留此诊断。",
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

function filterDiagnosticGroups(groups: DiagnosticGroup[], filter: string, searchQuery: string): DiagnosticGroup[] {
  const query = searchQuery.trim().toLowerCase();
  return groups.filter((group) => {
    if (filter === "error" && group.severity !== "error") return false;
    if (filter === "warning" && group.severity !== "warning") return false;
    if (filter === "lossy" && !group.lossy) return false;
    if (query) {
      const matchCode = group.code.toLowerCase().includes(query);
      const matchMsg = group.message.toLowerCase().includes(query);
      const matchLocations = group.diagnostics.some((d) => diagnosticLocation(d).toLowerCase().includes(query));
      if (!matchCode && !matchMsg && !matchLocations) return false;
    }
    return true;
  });
}

function renderDiagnostics(groups: DiagnosticGroup[]): void {
  diagnosticList.replaceChildren();
  const filtered = filterDiagnosticGroups(groups, currentDiagnosticFilter, currentDiagnosticSearchQuery);
  const visible = filtered.slice(0, 100);

  if (filtered.length === 0) {
    const empty = document.createElement("div");
    empty.className = "diagnostic-item";
    empty.style.justifyContent = "center";
    empty.style.color = "var(--muted)";
    empty.style.fontSize = "12px";
    empty.textContent = groups.length === 0 ? "未产生任何诊断信息，转换完全清洁。" : "没有匹配当前筛选条件的诊断记录。";
    diagnosticList.append(empty);
    return;
  }

  for (const group of visible) {
    const item = document.createElement("div");
    item.className = "diagnostic-item";
    const level = document.createElement("span");
    level.className = "diag-level " + (group.lossy ? "lossy" : group.severity);
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
      summary.textContent = "受影响 " + locations.length + " 处（展开查看详情）";
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
  if (filtered.length > visible.length) {
    const omitted = document.createElement("div");
    omitted.className = "diagnostic-item";
    omitted.textContent = "还有 " + (filtered.length - visible.length) + " 类诊断，请查看 ZIP 内的 conversion-report.json。";
    diagnosticList.append(omitted);
  }
}

function generateMarkdownDiagnosticReport(): string {
  if (!latestConversionReport) return "没有可用的转换诊断报告。";
  const identity = objectValue(latestConversionReport.identity);
  const counts = objectValue(latestConversionReport.counts);
  const targetNamespace = String(identity.targetItemNamespace ?? "未知");
  const diagnostics = Array.isArray(latestConversionReport.diagnostics) ? latestConversionReport.diagnostics : [];
  const lines: string[] = [
    "# Nexo → CraftEngine 转换与审计报告",
    "",
    "- **目标命名空间**: `" + targetNamespace + "`",
    "- **转换状态**: " + (latestConversionReport.success ? "成功 ✓" : "失败 / 需检查 ✗"),
    "- **转换项目**: " + numberValue(counts.items) + " 物品, " + numberValue(counts.categories) + " 分类, " + numberValue(counts.furniture) + " 家具, " + numberValue(counts.resources) + " 资源",
    "- **诊断总计**: " + diagnostics.length + " 条",
    "",
    "## 诊断汇总",
    "",
  ];
  if (currentDiagnosticGroups.length === 0) {
    lines.push("未产生任何诊断记录，全部配置与资源引用均完美匹配。");
  } else {
    for (const group of currentDiagnosticGroups) {
      lines.push("### [" + group.severity.toUpperCase() + (group.lossy ? "/LOSSY" : "") + "] " + group.code + " (×" + group.diagnostics.length + ")");
      lines.push("- **说明**: " + group.message);
      const locations = Array.from(new Set(group.diagnostics.map(diagnosticLocation).filter(Boolean)));
      if (locations.length > 0) {
        lines.push("- **涉及位置** (" + locations.length + "):");
        for (const loc of locations.slice(0, 10)) lines.push("  - `" + loc + "`");
        if (locations.length > 10) lines.push("  - ... 及其他 " + (locations.length - 10) + " 处");
      }
      lines.push("");
    }
  }
  return lines.join("\n");
}

function renderResult(blob: Blob, report: ConversionReport): void {
  clearResultPanel();
  latestConversionReport = report;
  const counts = objectValue(report.counts);
  const diagnosticsCounts = objectValue(counts.diagnostics);
  const diagnostics = Array.isArray(report.diagnostics) ? report.diagnostics : [];
  currentDiagnosticGroups = groupDiagnostics(diagnostics);
  const errors = numberValue(diagnosticsCounts.error);
  const warnings = numberValue(diagnosticsCounts.warning);
  const lossy = numberValue(diagnosticsCounts.lossy);
  const success = report.success === true;
  const identity = objectValue(report.identity);
  const detectedNamespace = typeof identity.targetItemNamespace === "string" ? identity.targetItemNamespace : "未知";

  resultPanel.classList.remove("is-hidden");
  resultPanel.classList.toggle("is-partial", !success && blob.size > 0);
  resultEyebrow.textContent = success ? "04 / COMPLETE" : "04 / REVIEW REQUIRED";
  resultTitle.textContent = success ? "转换完成" : "已生成结果，但需要检查诊断";
  resultMessage.textContent = success
    ? currentDiagnosticGroups.length === 0
      ? "已按作者命名空间 " + detectedNamespace + " 生成 CraftEngine 26.8 标准 ZIP（内部为 resources/" + detectedNamespace + " 结构），可直接解压到 plugins/CraftEngine/。"
      : "转换成功；" + diagnostics.length + " 条诊断已合并为 " + currentDiagnosticGroups.length + " 类。ZIP 内部已按 resources/" + detectedNamespace + " 标准打包。"
    : "报告中存在错误或严格模式不接受的有损项；仍可下载结果包进行排查。";
  resultMark.textContent = success ? "✓" : "!";
  resultBlobUrl = URL.createObjectURL(blob);
  const sourceStem = selectedSource?.name.replace(/\.zip$/i, "").replace(/[^\p{L}\p{N}._-]+/gu, "_").replace(/^_+|_+$/g, "") || "nexo-pack";
  resultFilename = "craftengine-" + sourceStem + ".zip";
  downloadButton.classList.remove("is-hidden");
  downloadButton.innerHTML = '<svg viewBox="0 0 20 20" width="18" height="18" fill="currentColor" aria-hidden="true"><path fill-rule="evenodd" d="M3 17a1 1 0 0 1 1-1h12a1 1 0 1 1 0 2H4a1 1 0 0 1-1-1zm3.293-7.707a1 1 0 0 1 1.414 0L9 10.586V3a1 1 0 1 1 2 0v7.586l1.293-1.293a1 1 0 1 1 1.414 1.414l-3 3a1 1 0 0 1-1.414 0l-3-3a1 1 0 0 1 0-1.414z" clip-rule="evenodd"/></svg><span>下载 CraftEngine ZIP (' + formatBytes(blob.size) + ")</span>";

  appendStat(numberValue(counts.items), "物品配置");
  appendStat(numberValue(counts.categories), "分类目录");
  appendStat(numberValue(counts.furniture), "家具模型");
  appendStat(numberValue(counts.blocks), "自定义方块");
  appendStat(numberValue(counts.recipes), "配方规则");
  appendStat(numberValue(counts.resources), "资源文件");
  appendStat(diagnostics.length, "诊断总数");

  const audit = objectValue(report.audit);
  auditSummary.replaceChildren();
  appendAudit("已解析模型", numberValue(audit.resolvedModels));
  appendAudit("缺失模型", numberValue(audit.missingModels));
  appendAudit("已解析纹理", numberValue(audit.resolvedTextures));
  appendAudit("缺失纹理", numberValue(audit.missingTextures));
  appendAudit("缺失 Blueprint", numberValue(audit.missingBlueprints));
  const auditFailures = numberValue(audit.missingModels) + numberValue(audit.missingTextures) + numberValue(audit.missingBlueprints);
  auditBadge.textContent = Object.keys(audit).length === 0 ? "未运行" : auditFailures === 0 ? "全量通过 ✓" : auditFailures + " 项缺失";
  auditBadge.style.color = auditFailures === 0 ? "var(--green-bright)" : "var(--amber)";

  diagnosticSummary.replaceChildren();
  appendDiagnosticMetric(errors, "错误", "error");
  appendDiagnosticMetric(warnings, "警告", "warning");
  appendDiagnosticMetric(lossy, "有损映射", "lossy");
  diagnosticBadge.textContent = currentDiagnosticGroups.length === diagnostics.length ? String(diagnostics.length) : currentDiagnosticGroups.length + " 类";
  diagnosticCount.textContent = currentDiagnosticGroups.length === diagnostics.length ? String(diagnostics.length) : currentDiagnosticGroups.length + " 类 / " + diagnostics.length + " 项";
  diagnosticDetails.classList.toggle("is-hidden", diagnostics.length === 0);
  renderDiagnostics(currentDiagnosticGroups);
  updateSteps("download");
  showToast(success ? "🎉 转换成功，已生成 CraftEngine ZIP" : "⚠️ 转换生成完毕，请复核诊断", success);
  window.setTimeout(() => resultTitle.focus(), 50);
  resultPanel.scrollIntoView({ behavior: "smooth", block: "start" });
}

function renderFatalError(error: unknown): void {
  resultPanel.classList.remove("is-hidden", "is-partial");
  resultPanel.classList.add("is-failed");
  resultTitle.textContent = "转换失败";
  resultMessage.textContent = error instanceof Error ? error.message : String(error);
  resultEyebrow.textContent = "ERROR / FAILED";
  resultMark.textContent = "×";
  downloadButton.classList.add("is-hidden");
  resultStats.replaceChildren();
  auditSummary.replaceChildren();
  diagnosticSummary.replaceChildren();
  diagnosticDetails.classList.add("is-hidden");
  showToast("转换未能完成：" + (error instanceof Error ? error.message : String(error)), false);
  window.setTimeout(() => resultTitle.focus(), 50);
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
      showToast("已取消转换");
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
  showToast("开始下载：" + resultFilename);
}

// Event Listeners
chooseZipButton.addEventListener("click", (event) => { event.stopPropagation(); zipInput.click(); });
chooseFolderButton.addEventListener("click", (event) => { event.stopPropagation(); folderInput.click(); });
changeInputButton.addEventListener("click", () => clearSource(false));
clearButton.addEventListener("click", () => clearSource());
convertAnotherButton.addEventListener("click", () => {
  clearSource();
  dropZone.scrollIntoView({ behavior: "smooth", block: "center" });
});
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

for (const name of ["dragenter", "dragover"]) {
  dropZone.addEventListener(name, (event) => {
    event.preventDefault();
    dropZone.classList.add("is-dragging");
  });
}

for (const name of ["dragleave", "drop"]) {
  dropZone.addEventListener(name, (event) => {
    event.preventDefault();
    dropZone.classList.remove("is-dragging");
  });
}

dropZone.addEventListener("drop", (event) => {
  if (!event.dataTransfer) return;
  void handleDrop(event.dataTransfer).catch((error) => showInputError(error instanceof Error ? error.message : String(error)));
});

// Diagnostic search & filter tabs
document.querySelectorAll<HTMLButtonElement>(".filter-tab").forEach((tab) => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".filter-tab").forEach((t) => t.classList.remove("is-active"));
    tab.classList.add("is-active");
    currentDiagnosticFilter = tab.dataset.filter ?? "all";
    renderDiagnostics(currentDiagnosticGroups);
  });
});

diagnosticSearch?.addEventListener("input", () => {
  currentDiagnosticSearchQuery = diagnosticSearch.value;
  renderDiagnostics(currentDiagnosticGroups);
});

copyDiagnosticsButton?.addEventListener("click", async (event) => {
  event.stopPropagation();
  const text = generateMarkdownDiagnosticReport();
  try {
    await navigator.clipboard.writeText(text);
    showToast("✓ 诊断报告已复制到剪贴板");
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.style.position = "fixed";
    textarea.style.opacity = "0";
    document.body.append(textarea);
    textarea.select();
    document.execCommand("copy");
    textarea.remove();
    showToast("✓ 诊断报告已复制到剪贴板");
  }
});

// Global Keyboard Shortcuts & Paste Handling
window.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    if (selectedSource?.valid && !busy) {
      event.preventDefault();
      void runConversion();
    }
  } else if (event.key === "Escape") {
    if (busy) {
      event.preventDefault();
      cancelButton.click();
    }
  }
});

window.addEventListener("paste", (event) => {
  const items = event.clipboardData?.items;
  if (!items) return;
  for (const item of Array.from(items)) {
    if (item.kind === "file") {
      const file = item.getAsFile();
      if (file && file.name.toLowerCase().endsWith(".zip")) {
        event.preventDefault();
        setZipSource(file);
        break;
      }
    }
  }
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
