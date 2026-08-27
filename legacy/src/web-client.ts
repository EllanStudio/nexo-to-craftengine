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

interface ConversionReport {
  success?: boolean;
  counts?: Record<string, unknown>;
  audit?: Record<string, unknown>;
  diagnostics?: Array<Record<string, unknown>>;
  web?: Record<string, unknown>;
  identity?: Record<string, unknown>;
}

export interface DiagnosticGroup {
  code: string;
  severity: string;
  lossy: boolean;
  message: string;
  diagnostics: Array<Record<string, unknown>>;
}

export interface QueueItem {
  id: string;
  kind: "zip" | "folder";
  name: string;
  file?: File;
  folderEntries?: InputEntry[];
  bytes: number;
  detectedRoot?: string;
  status: "pending" | "packaging" | "converting" | "success" | "error" | "cancelled";
  errorMessage?: string;
  resultBlob?: Blob;
  resultBlobUrl?: string;
  resultFilename?: string;
  report?: ConversionReport;
  itemCount?: number;
  categoryCount?: number;
  furnitureCount?: number;
  blockCount?: number;
  recipeCount?: number;
  soundCount?: number;
  glyphCount?: number;
  resourceCount?: number;
  diagnosticsCount?: number;
  errorCount?: number;
  warningCount?: number;
  lossyCount?: number;
  diagnosticsGroups?: DiagnosticGroup[];
}

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

const byId = <T extends HTMLElement>(id: string): T => {
  const element = document.getElementById(id);
  if (!element) throw new Error("Missing Web UI element: " + id);
  return element as T;
};

// Section blocks
const dropSection = byId<HTMLElement>("dropSection");
const queueSection = byId<HTMLElement>("queueSection");
const dropZone = byId<HTMLDivElement>("dropZone");
const zipInput = byId<HTMLInputElement>("zipInput");
const folderInput = byId<HTMLInputElement>("folderInput");
const chooseZipButton = byId<HTMLButtonElement>("chooseZipButton");
const chooseFolderButton = byId<HTMLButtonElement>("chooseFolderButton");
const addMoreZipButton = byId<HTMLButtonElement>("addMoreZipButton");
const addMoreFolderButton = byId<HTMLButtonElement>("addMoreFolderButton");
const clearQueueButton = byId<HTMLButtonElement>("clearQueueButton");
const queueStatsBadge = byId<HTMLElement>("queueStatsBadge");
const queueList = byId<HTMLElement>("queueList");
const queueSummaryLine = byId<HTMLElement>("queueSummaryLine");

// Action & Progress elements
const batchProgressContainer = byId<HTMLElement>("batchProgressContainer");
const liveStatus = byId<HTMLElement>("liveStatus");
const progressBarFill = byId<HTMLElement>("progressBarFill");
const elapsedTime = byId<HTMLElement>("elapsedTime");
const cancelButton = byId<HTMLButtonElement>("cancelButton");
const convertButton = byId<HTMLButtonElement>("convertButton");
const convertBtnText = byId<HTMLElement>("convertBtnText");
const downloadAllButton = byId<HTMLButtonElement>("downloadAllButton");
const inputError = byId<HTMLParagraphElement>("inputError");

// Options elements
const clientModeSelect = byId<HTMLSelectElement>("clientModeSelect");
const cmdPolicySelect = byId<HTMLSelectElement>("cmdPolicySelect");
const strictInput = byId<HTMLInputElement>("strictInput");
const auditInput = byId<HTMLInputElement>("auditInput");
const optionsFieldset = byId<HTMLFieldSetElement>("optionsFieldset");

// Modal elements
const diagnosticsModal = byId<HTMLDivElement>("diagnosticsModal");
const modalTitle = byId<HTMLElement>("modalTitle");
const modalSubtitle = byId<HTMLElement>("modalSubtitle");
const closeModalButton = byId<HTMLButtonElement>("closeModalButton");
const modalCloseButton = byId<HTMLButtonElement>("modalCloseButton");
const modalDownloadButton = byId<HTMLButtonElement>("modalDownloadButton");
const resultStats = byId<HTMLElement>("resultStats");
const diagnosticBadge = byId<HTMLElement>("diagnosticBadge");
const diagnosticSummary = byId<HTMLElement>("diagnosticSummary");
const diagnosticDetails = byId<HTMLDetailsElement>("diagnosticDetails");
const diagnosticCount = byId<HTMLElement>("diagnosticCount");
const diagnosticList = byId<HTMLElement>("diagnosticList");
const copyDiagnosticsButton = byId<HTMLButtonElement>("copyDiagnosticsButton");
const diagnosticSearch = byId<HTMLInputElement>("diagnosticSearch");
const toastContainer = byId<HTMLDivElement>("toastContainer");

// Application State
const queue: QueueItem[] = [];
let activeRequest: XMLHttpRequest | undefined;
let activeModalItem: QueueItem | undefined;
let operationStarted = 0;
let elapsedTimer: number | undefined;
let operationGeneration = 0;
let maxUploadBytes = 256 * 1024 * 1024;
let maxExpandedBytes = 512 * 1024 * 1024;
let maxFileBytes = 128 * 1024 * 1024;
let maxSourceFiles = 25_000;
let isBusy = false;
let currentDiagnosticFilter = "all";
let currentDiagnosticSearchQuery = "";

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

function showInputError(message?: string): void {
  inputError.textContent = message ?? "";
  inputError.classList.toggle("is-hidden", !message);
}

function normalizeClientPath(path: string): string {
  const normalized = path.replaceAll("\\", "/").replace(/^\/+/, "");
  const segments = normalized.split("/");
  if (!normalized || segments.some((segment) => !segment || segment === "." || segment === "..")) {
    throw new Error("包含不安全路径：" + path);
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
  if (roots.size === 0) return { error: "未找到包含 .yml 的 items/ 目录", itemFiles, assets };
  const rootList = Array.from(roots);
  const explicitNexo = rootList.filter((root) => root !== "." && root.split("/").at(-1)?.toLowerCase() === "nexo");
  const root = explicitNexo[0] ?? rootList[0]!;
  const prefix = root === "." ? "" : root + "/";
  assets = entries.filter((entry) => {
    const path = entry.relativePath.slice(prefix.length).toLowerCase();
    return entry.relativePath.startsWith(prefix) && (path.startsWith("pack/assets/") || path.startsWith("resourcepack/assets/") || path.startsWith("assets/"));
  }).length;
  return { root, itemFiles, assets };
}

function addZipFiles(files: File[]): void {
  let addedCount = 0;
  for (const file of files) {
    if (!file.name.toLowerCase().endsWith(".zip")) continue;
    if (file.size > maxUploadBytes) {
      showToast(`文件 ${file.name} 超过大小限制 (${formatBytes(maxUploadBytes)})`, false);
      continue;
    }
    const item: QueueItem = {
      id: "zip-" + Math.random().toString(36).slice(2, 9),
      kind: "zip",
      name: file.name,
      file,
      bytes: file.size,
      status: "pending",
    };
    queue.push(item);
    addedCount++;
  }
  if (addedCount > 0) {
    showToast(`已添加 ${addedCount} 个 ZIP 文件到转换队列`);
    renderQueue();
  }
}

function addFolder(entries: InputEntry[], name: string): void {
  try {
    const collisions = new Set<string>();
    const normalizedEntries = entries.map((entry) => {
      const path = normalizeClientPath(entry.relativePath);
      const key = path.normalize("NFC").toLowerCase();
      if (collisions.has(key)) throw new Error("来源包含重复路径：" + path);
      collisions.add(key);
      return { relativePath: path, file: entry.file };
    });
    const bytes = normalizedEntries.reduce((sum, entry) => sum + entry.file.size, 0);
    const detection = detectFolderRoot(normalizedEntries);
    const item: QueueItem = {
      id: "folder-" + Math.random().toString(36).slice(2, 9),
      kind: "folder",
      name,
      folderEntries: normalizedEntries,
      bytes,
      detectedRoot: detection.root,
      status: "pending",
      errorMessage: detection.error,
    };
    queue.push(item);
    showToast(`已添加文件夹 [${name}] 到转换队列`);
    renderQueue();
  } catch (error) {
    showInputError(error instanceof Error ? error.message : String(error));
  }
}

function renderQueue(): void {
  if (queue.length === 0) {
    dropSection.classList.remove("is-hidden");
    queueSection.classList.add("is-hidden");
    return;
  }

  dropSection.classList.add("is-hidden");
  queueSection.classList.remove("is-hidden");

  const totalBytes = queue.reduce((sum, item) => sum + item.bytes, 0);
  const successCount = queue.filter((item) => item.status === "success").length;
  const errorCount = queue.filter((item) => item.status === "error").length;
  const pendingCount = queue.filter((item) => item.status === "pending" || item.status === "converting" || item.status === "packaging").length;

  queueStatsBadge.textContent = `共 ${queue.length} 个文件 · ${formatBytes(totalBytes)}`;
  
  if (isBusy) {
    queueSummaryLine.textContent = `正在转换中 (待处理 ${pendingCount} 项)...`;
  } else if (successCount + errorCount === queue.length) {
    queueSummaryLine.textContent = `转换完毕：成功 ${successCount} 个${errorCount > 0 ? `，失败 ${errorCount} 个` : ""}`;
  } else {
    queueSummaryLine.textContent = `就绪：共 ${queue.length} 个文件，随时可以开始转换`;
  }

  convertButton.disabled = isBusy || pendingCount === 0;
  convertBtnText.textContent = queue.length > 1 ? `开始批量转换 (${pendingCount})` : "开始转换";
  cancelButton.classList.toggle("is-hidden", !isBusy);
  convertButton.classList.toggle("is-hidden", isBusy);
  downloadAllButton.classList.toggle("is-hidden", successCount === 0 || isBusy);
  clearQueueButton.disabled = isBusy;
  addMoreZipButton.disabled = isBusy;
  addMoreFolderButton.disabled = isBusy;

  queueList.replaceChildren();

  for (const item of queue) {
    const el = document.createElement("div");
    el.className = `queue-item is-${item.status}`;

    const mainRow = document.createElement("div");
    mainRow.className = "queue-item-main";

    // Left info
    const left = document.createElement("div");
    left.className = "queue-item-left";

    const icon = document.createElement("div");
    icon.className = "queue-item-icon";
    icon.textContent = item.kind === "folder" ? "DIR" : "ZIP";

    const info = document.createElement("div");
    info.className = "queue-item-info";

    const name = document.createElement("div");
    name.className = "queue-item-name";
    name.textContent = item.name;
    name.title = item.name;

    const meta = document.createElement("div");
    meta.className = "queue-item-meta";
    meta.textContent = `${formatBytes(item.bytes)}${item.detectedRoot ? ` · 根目录 [${item.detectedRoot}]` : ""}`;

    info.append(name, meta);
    left.append(icon, info);

    // Right status & actions
    const right = document.createElement("div");
    right.className = "queue-item-right";

    // Status badge
    const badge = document.createElement("span");
    if (item.status === "pending") {
      badge.className = "badge badge-pending";
      badge.textContent = "等待转换";
    } else if (item.status === "packaging" || item.status === "converting") {
      badge.className = "badge badge-converting";
      badge.textContent = item.status === "packaging" ? "打包中..." : "转换中...";
    } else if (item.status === "success") {
      badge.className = "badge badge-success";
      badge.textContent = item.diagnosticsCount && item.diagnosticsCount > 0 ? `完成 (${item.diagnosticsCount} 诊断)` : "转换成功";
    } else if (item.status === "error") {
      badge.className = "badge badge-error";
      badge.textContent = "转换失败";
    }
    right.append(badge);

    // Action buttons
    if (item.status === "success" && item.resultBlob) {
      const dlBtn = document.createElement("button");
      dlBtn.className = "btn btn-xs btn-primary";
      dlBtn.innerHTML = `⬇️ 下载 (${formatBytes(item.resultBlob.size)})`;
      dlBtn.onclick = (e) => {
        e.stopPropagation();
        downloadItemZip(item);
      };
      right.append(dlBtn);

      const detailBtn = document.createElement("button");
      detailBtn.className = "btn btn-xs btn-ghost";
      detailBtn.textContent = "详情";
      detailBtn.onclick = (e) => {
        e.stopPropagation();
        openModal(item);
      };
      right.append(detailBtn);
    } else if (item.status === "error") {
      const detailBtn = document.createElement("button");
      detailBtn.className = "btn btn-xs btn-ghost";
      detailBtn.textContent = "查看原因";
      detailBtn.onclick = (e) => {
        e.stopPropagation();
        openModal(item);
      };
      right.append(detailBtn);
    }

    if (!isBusy && (item.status === "pending" || item.status === "success" || item.status === "error")) {
      const removeBtn = document.createElement("button");
      removeBtn.className = "btn btn-xs btn-ghost";
      removeBtn.innerHTML = "✕";
      removeBtn.title = "从队列移除";
      removeBtn.onclick = (e) => {
        e.stopPropagation();
        const idx = queue.findIndex((q) => q.id === item.id);
        if (idx !== -1) queue.splice(idx, 1);
        renderQueue();
      };
      right.append(removeBtn);
    }

    mainRow.append(left, right);
    el.append(mainRow);

    // Converted type chips (转换种类) if success
    if (item.status === "success" && item.report) {
      const chips = document.createElement("div");
      chips.className = "queue-item-chips";

      const appendChip = (icon: string, label: string, count?: number) => {
        if (count && count > 0) {
          const chip = document.createElement("span");
          chip.className = "type-chip highlight";
          chip.innerHTML = `${icon} ${label} <strong>${count}</strong>`;
          chips.append(chip);
        }
      };

      appendChip("📦", "物品", item.itemCount);
      appendChip("🪑", "家具", item.furnitureCount);
      appendChip("🧱", "方块", item.blockCount);
      appendChip("📁", "分类", item.categoryCount);
      appendChip("📜", "配方", item.recipeCount);
      appendChip("🔊", "音效", item.soundCount);
      appendChip("🔤", "字符", item.glyphCount);
      appendChip("🎨", "材质", item.resourceCount);

      if (chips.children.length > 0) {
        el.append(chips);
      }
    } else if (item.status === "error" && item.errorMessage) {
      const errBox = document.createElement("div");
      errBox.className = "queue-item-error";
      errBox.textContent = "❌ " + item.errorMessage;
      el.append(errBox);
    }

    queueList.append(el);
  }
}

function clearQueue(): void {
  if (isBusy) return;
  for (const item of queue) {
    if (item.resultBlobUrl) URL.revokeObjectURL(item.resultBlobUrl);
  }
  queue.length = 0;
  zipInput.value = "";
  folderInput.value = "";
  renderQueue();
}

function downloadItemZip(item: QueueItem): void {
  if (!item.resultBlob) return;
  const url = item.resultBlobUrl ?? URL.createObjectURL(item.resultBlob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = item.resultFilename ?? "craftengine-output.zip";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  showToast(`开始下载：${anchor.download}`);
}

async function downloadAllMasterZip(): Promise<void> {
  const successItems = queue.filter((item) => item.status === "success" && item.resultBlob);
  if (successItems.length === 0) return;
  
  if (successItems.length === 1) {
    downloadItemZip(successItems[0]!);
    return;
  }

  showToast("正在打包所有转换结果为总压缩包...");
  const masterZip: Record<string, Uint8Array> = {};
  for (const item of successItems) {
    if (item.resultBlob) {
      const filename = item.resultFilename ?? `craftengine-${item.name}.zip`;
      masterZip[filename] = new Uint8Array(await item.resultBlob.arrayBuffer());
    }
  }

  const bundled = window.fflate.zipSync(masterZip, { level: 0 });
  const blob = new Blob([bundled.buffer as ArrayBuffer], { type: "application/zip" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `craftengine-batch-${new Date().toISOString().slice(0, 10)}.zip`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  showToast(`✓ 已打包下载全部 ${successItems.length} 个文件`);
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
  const zipFiles = ordinaryFiles.filter((f) => f.name.toLowerCase().endsWith(".zip"));
  
  if (zipFiles.length > 0 && zipFiles.length === ordinaryFiles.length) {
    addZipFiles(zipFiles);
    return;
  }

  // Handle directory drop
  const transferItems = Array.from(dataTransfer.items);
  const folderBuckets = new Map<string, InputEntry[]>();

  for (const item of transferItems) {
    const legacy = (item as DataTransferItem & { webkitGetAsEntry?: () => LegacyEntry | null }).webkitGetAsEntry?.();
    if (legacy) {
      if (legacy.isDirectory) {
        const entries: InputEntry[] = [];
        await traverseLegacyEntry(legacy, "", entries);
        if (entries.length > 0) folderBuckets.set(legacy.name, entries);
      } else if (legacy.isFile && legacy.name.toLowerCase().endsWith(".zip")) {
        const file = await legacyFile(legacy as unknown as LegacyFileEntry);
        addZipFiles([file]);
      }
    }
  }

  for (const [folderName, entries] of folderBuckets) {
    addFolder(entries, folderName);
  }
}

function setBusy(value: boolean): void {
  isBusy = value;
  optionsFieldset.disabled = value;
  chooseFolderButton.disabled = value;
  chooseZipButton.disabled = value;
  clearQueueButton.disabled = value;
  addMoreZipButton.disabled = value;
  addMoreFolderButton.disabled = value;
  batchProgressContainer.classList.toggle("is-hidden", !value);
  cancelButton.classList.toggle("is-hidden", !value);
  convertButton.classList.toggle("is-hidden", value);
  renderQueue();
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

async function packageFolder(item: QueueItem, generation: number): Promise<Blob> {
  const entries = item.folderEntries ?? [];
  const files: Record<string, Uint8Array> = {};
  let completedBytes = 0;
  for (let index = 0; index < entries.length; index++) {
    if (generation !== operationGeneration) throw new DOMException("Conversion cancelled", "AbortError");
    const entry = entries[index]!;
    files[entry.relativePath] = new Uint8Array(await entry.file.arrayBuffer());
    completedBytes += entry.file.size;
  }
  await nextFrame();
  const zipped = window.fflate.zipSync(files, { level: 0 });
  if (zipped.byteLength > maxUploadBytes) throw new Error("打包后的 ZIP 超过上传限制（" + formatBytes(maxUploadBytes) + "）");
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
    
    xhr.addEventListener("load", () => {
      if (generation !== operationGeneration) return;
      activeRequest = undefined;
      const response = xhr.response as Blob;
      if (xhr.status >= 200 && xhr.status < 300 && (response.type === "application/zip" || xhr.getResponseHeader("Content-Type")?.startsWith("application/zip"))) {
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

    xhr.addEventListener("error", () => rejectUpload(new Error("无法连接本机转换服务，请确认服务仍在运行")));
    xhr.addEventListener("abort", () => rejectUpload(new DOMException("Conversion cancelled", "AbortError")));
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

function diagnosticLocation(diagnostic: Record<string, unknown>): string {
  return [diagnostic.source, diagnostic.item, diagnostic.field].filter((value) => typeof value === "string" && value.length > 0).join(" / ");
}

export function groupDiagnostics(diagnostics: Array<Record<string, unknown>>): DiagnosticGroup[] {
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

export function filterDiagnosticGroups(groups: DiagnosticGroup[], filter: string, searchQuery: string): DiagnosticGroup[] {
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
    empty.style.color = "var(--text-muted)";
    empty.textContent = groups.length === 0 ? "未产生任何诊断记录，全部无损匹配。" : "没有匹配当前筛选条件的记录。";
    diagnosticList.append(empty);
    return;
  }

  for (const group of visible) {
    const item = document.createElement("div");
    item.className = "diagnostic-item";
    
    const badge = document.createElement("span");
    const levelClass = group.lossy ? "lossy" : group.severity;
    badge.className = "diag-level-badge " + levelClass;
    badge.textContent = (group.severity === "error" ? "错误" : group.severity === "warning" ? "警告" : "信息") + (group.lossy ? " · 有损" : "") + (group.diagnostics.length > 1 ? " ×" + group.diagnostics.length : "");
    
    const body = document.createElement("div");
    body.className = "diag-content";
    const code = document.createElement("strong");
    code.textContent = group.code;
    const msg = document.createElement("p");
    msg.textContent = group.message;
    body.append(code, msg);

    const locations = Array.from(new Set(group.diagnostics.map(diagnosticLocation).filter(Boolean)));
    if (locations.length === 1) {
      const path = document.createElement("small");
      path.textContent = locations[0]!;
      body.append(path);
    } else if (locations.length > 1) {
      const occurrences = document.createElement("details");
      occurrences.className = "diag-occurrences";
      const summary = document.createElement("summary");
      summary.textContent = "涉及 " + locations.length + " 处位置";
      const list = document.createElement("ul");
      for (const loc of locations.slice(0, 50)) {
        const row = document.createElement("li");
        row.textContent = loc;
        list.append(row);
      }
      occurrences.append(summary, list);
      body.append(occurrences);
    }

    item.append(badge, body);
    diagnosticList.append(item);
  }
}

function appendTypeCard(icon: string, label: string, count: number): void {
  const card = document.createElement("div");
  card.className = "type-card" + (count > 0 ? " has-items" : "");
  
  const header = document.createElement("div");
  header.className = "type-header";
  header.innerHTML = `<span>${icon}</span> <span>${label}</span>`;
  
  const number = document.createElement("div");
  number.className = "type-count";
  number.textContent = String(count);
  
  card.append(header, number);
  resultStats.append(card);
}

function openModal(item: QueueItem): void {
  activeModalItem = item;
  modalTitle.textContent = item.name;
  modalSubtitle.textContent = item.status === "success" ? "转换成功 · 种类与诊断统计" : item.status === "error" ? "转换失败 · 错误详情" : "待转换";

  // Render Converted Types Grid (转换的种类有啥)
  resultStats.replaceChildren();
  appendTypeCard("📦", "物品配置", item.itemCount ?? 0);
  appendTypeCard("🪑", "家具模型", item.furnitureCount ?? 0);
  appendTypeCard("🧱", "自定义方块", item.blockCount ?? 0);
  appendTypeCard("📁", "分类目录", item.categoryCount ?? 0);
  appendTypeCard("📜", "合成配方", item.recipeCount ?? 0);
  appendTypeCard("🔊", "音效配置", item.soundCount ?? 0);
  appendTypeCard("🔤", "特殊字符", item.glyphCount ?? 0);
  appendTypeCard("🎨", "材质资源", item.resourceCount ?? 0);

  // Render Diagnostics Summary
  diagnosticSummary.replaceChildren();
  if (item.errorCount && item.errorCount > 0) {
    const pill = document.createElement("span");
    pill.className = "diag-pill error";
    pill.textContent = `❌ ${item.errorCount} 个错误`;
    diagnosticSummary.append(pill);
  }
  if (item.warningCount && item.warningCount > 0) {
    const pill = document.createElement("span");
    pill.className = "diag-pill warning";
    pill.textContent = `⚠️ ${item.warningCount} 个警告`;
    diagnosticSummary.append(pill);
  }
  if (item.lossyCount && item.lossyCount > 0) {
    const pill = document.createElement("span");
    pill.className = "diag-pill lossy";
    pill.textContent = `⚡ ${item.lossyCount} 个有损映射`;
    diagnosticSummary.append(pill);
  }
  if (item.status === "error" && item.errorMessage) {
    const pill = document.createElement("span");
    pill.className = "diag-pill error";
    pill.textContent = "❌ " + item.errorMessage;
    diagnosticSummary.append(pill);
  } else if (item.status === "success" && (item.diagnosticsCount ?? 0) === 0) {
    const pill = document.createElement("span");
    pill.className = "diag-pill";
    pill.style.background = "var(--success-bg)";
    pill.style.color = "var(--success)";
    pill.style.border = "1px solid var(--success-border)";
    pill.textContent = "✓ 无诊断异常，完美转换";
    diagnosticSummary.append(pill);
  }

  const groups = item.diagnosticsGroups ?? [];
  diagnosticBadge.textContent = String(item.diagnosticsCount ?? 0);
  diagnosticCount.textContent = groups.length === (item.diagnosticsCount ?? 0) ? String(item.diagnosticsCount ?? 0) : `${groups.length} 类 / ${item.diagnosticsCount ?? 0} 项`;
  renderDiagnostics(groups);

  modalDownloadButton.classList.toggle("is-hidden", item.status !== "success");
  diagnosticsModal.classList.remove("is-hidden");
}

function closeModal(): void {
  diagnosticsModal.classList.add("is-hidden");
  activeModalItem = undefined;
}

export async function runConversion(): Promise<void> {
  const pendingItems = queue.filter((item) => item.status === "pending");
  if (pendingItems.length === 0 || isBusy) return;

  const generation = ++operationGeneration;
  setBusy(true);
  startElapsedTimer();

  let completedCount = 0;
  const total = pendingItems.length;

  for (let index = 0; index < total; index++) {
    if (generation !== operationGeneration) break;
    const item = pendingItems[index]!;
    item.status = "converting";
    renderQueue();

    liveStatus.textContent = `正在转换 (${index + 1} / ${total}): ${item.name}...`;
    progressBarFill.style.width = `${Math.round((index / total) * 100)}%`;

    try {
      let upload: Blob;
      if (item.kind === "folder") {
        item.status = "packaging";
        renderQueue();
        upload = await packageFolder(item, generation);
        item.status = "converting";
        renderQueue();
      } else {
        upload = item.file!;
      }

      if (generation !== operationGeneration) throw new DOMException("Conversion cancelled", "AbortError");
      const output = await uploadZip(upload, generation);
      const inspected = await inspectOutput(output);
      if (generation !== operationGeneration) return;

      const report = inspected.report;
      const counts = objectValue(report.counts);
      const diagnosticsCounts = objectValue(counts.diagnostics);
      const diagnostics = Array.isArray(report.diagnostics) ? report.diagnostics : [];
      const groups = groupDiagnostics(diagnostics);
      const identity = objectValue(report.identity);
      const detectedNamespace = typeof identity.targetItemNamespace === "string" ? identity.targetItemNamespace : "craftengine";

      item.status = "success";
      item.report = report;
      item.resultBlob = output;
      item.resultBlobUrl = URL.createObjectURL(output);
      const sourceStem = item.name.replace(/\.zip$/i, "").replace(/[^\p{L}\p{N}._-]+/gu, "_").replace(/^_+|_+$/g, "") || detectedNamespace;
      item.resultFilename = `craftengine-${sourceStem}.zip`;
      item.itemCount = numberValue(counts.items);
      item.categoryCount = numberValue(counts.categories);
      item.furnitureCount = numberValue(counts.furniture);
      item.blockCount = numberValue(counts.blocks);
      item.recipeCount = numberValue(counts.recipes);
      item.soundCount = numberValue(counts.sounds);
      item.glyphCount = numberValue(counts.glyphs);
      item.resourceCount = numberValue(counts.resources);
      item.diagnosticsCount = diagnostics.length;
      item.errorCount = numberValue(diagnosticsCounts.error);
      item.warningCount = numberValue(diagnosticsCounts.warning);
      item.lossyCount = numberValue(diagnosticsCounts.lossy);
      item.diagnosticsGroups = groups;

      completedCount++;
    } catch (error) {
      if (generation !== operationGeneration || (error instanceof DOMException && error.name === "AbortError")) {
        item.status = "pending";
        break;
      } else {
        item.status = "error";
        item.errorMessage = error instanceof Error ? error.message : String(error);
      }
    }

    renderQueue();
  }

  progressBarFill.style.width = "100%";
  stopElapsedTimer();
  setBusy(false);

  if (generation === operationGeneration) {
    activeRequest = undefined;
    const successes = queue.filter((i) => i.status === "success").length;
    showToast(`批量转换完成：成功 ${successes} / ${queue.length} 个文件`, successes > 0);
  }
}

// Event Listeners
chooseZipButton.addEventListener("click", () => zipInput.click());
chooseFolderButton.addEventListener("click", () => folderInput.click());
addMoreZipButton.addEventListener("click", () => zipInput.click());
addMoreFolderButton.addEventListener("click", () => folderInput.click());
clearQueueButton.addEventListener("click", clearQueue);
convertButton.addEventListener("click", () => { void runConversion(); });
downloadAllButton.addEventListener("click", () => { void downloadAllMasterZip(); });

cancelButton.addEventListener("click", () => {
  operationGeneration++;
  activeRequest?.abort();
  activeRequest = undefined;
  stopElapsedTimer();
  setBusy(false);
  showToast("已取消转换任务");
});

closeModalButton.addEventListener("click", closeModal);
modalCloseButton.addEventListener("click", closeModal);
modalDownloadButton.addEventListener("click", () => {
  if (activeModalItem) downloadItemZip(activeModalItem);
});

diagnosticsModal.addEventListener("click", (event) => {
  if (event.target === diagnosticsModal) closeModal();
});

zipInput.addEventListener("change", () => {
  const files = Array.from(zipInput.files ?? []);
  if (files.length > 0) addZipFiles(files);
  zipInput.value = "";
});

folderInput.addEventListener("change", () => {
  const files = Array.from(folderInput.files ?? []);
  if (files.length === 0) return;
  const entries = files.map((file) => ({ relativePath: file.webkitRelativePath || file.name, file }));
  const rootName = entries[0]!.relativePath.split("/")[0] ?? "Nexo";
  addFolder(entries, rootName);
  folderInput.value = "";
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
document.querySelectorAll<HTMLButtonElement>(".pill-tab").forEach((tab) => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".pill-tab").forEach((t) => t.classList.remove("is-active"));
    tab.classList.add("is-active");
    currentDiagnosticFilter = tab.dataset.filter ?? "all";
    if (activeModalItem) renderDiagnostics(activeModalItem.diagnosticsGroups ?? []);
  });
});

diagnosticSearch?.addEventListener("input", () => {
  currentDiagnosticSearchQuery = diagnosticSearch.value;
  if (activeModalItem) renderDiagnostics(activeModalItem.diagnosticsGroups ?? []);
});

copyDiagnosticsButton?.addEventListener("click", async (event) => {
  event.stopPropagation();
  if (!activeModalItem) return;
  const item = activeModalItem;
  const groups = item.diagnosticsGroups ?? [];
  const lines: string[] = [
    `# Nexo → CraftEngine 转换报告 - ${item.name}`,
    "",
    `- **转换状态**: ${item.status === "success" ? "成功 ✓" : "失败 ✗"}`,
    `- **转换项目**: ${item.itemCount ?? 0} 物品, ${item.categoryCount ?? 0} 分类, ${item.furnitureCount ?? 0} 家具, ${item.resourceCount ?? 0} 资源`,
    `- **诊断总计**: ${item.diagnosticsCount ?? 0} 条`,
    "",
    "## 诊断详情",
    "",
  ];
  if (groups.length === 0) {
    lines.push("未产生任何诊断记录，全部配置与资源引用均完美转换。");
  } else {
    for (const group of groups) {
      lines.push(`### [${group.severity.toUpperCase()}${group.lossy ? "/LOSSY" : ""}] ${group.code} (×${group.diagnostics.length})`);
      lines.push(`- **说明**: ${group.message}`);
      const locations = Array.from(new Set(group.diagnostics.map(diagnosticLocation).filter(Boolean)));
      if (locations.length > 0) {
        lines.push(`- **涉及位置** (${locations.length}):`);
        for (const loc of locations.slice(0, 10)) lines.push(`  - \`${loc}\``);
        if (locations.length > 10) lines.push(`  - ... 及其他 ${locations.length - 10} 处`);
      }
      lines.push("");
    }
  }
  const text = lines.join("\n");
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

// Global Keyboard Shortcuts & Paste
window.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    if (!isBusy) {
      event.preventDefault();
      void runConversion();
    }
  } else if (event.key === "Escape") {
    if (!diagnosticsModal.classList.contains("is-hidden")) {
      closeModal();
    } else if (isBusy) {
      event.preventDefault();
      cancelButton.click();
    }
  }
});

window.addEventListener("paste", (event) => {
  const items = event.clipboardData?.items;
  if (!items) return;
  const zips: File[] = [];
  for (const item of Array.from(items)) {
    if (item.kind === "file") {
      const file = item.getAsFile();
      if (file && file.name.toLowerCase().endsWith(".zip")) {
        zips.push(file);
      }
    }
  }
  if (zips.length > 0) {
    event.preventDefault();
    addZipFiles(zips);
  }
});

window.addEventListener("beforeunload", () => {
  activeRequest?.abort();
  for (const item of queue) {
    if (item.resultBlobUrl) URL.revokeObjectURL(item.resultBlobUrl);
  }
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
    showInputError("无法连接本地服务；转换时将再次尝试");
  }
}

void loadHealth();
