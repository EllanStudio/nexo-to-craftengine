import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, relative, resolve, sep } from "node:path";
import { unzipSync, zipSync, type UnzipFileInfo, type Zippable } from "fflate";

export interface ArchiveLimits {
  maxUploadBytes: number;
  maxFiles: number;
  maxFileBytes: number;
  maxUncompressedBytes: number;
  maxCompressionRatio: number;
  maxPathLength: number;
  maxDepth: number;
}

export const DEFAULT_ARCHIVE_LIMITS: ArchiveLimits = {
  maxUploadBytes: 256 * 1024 * 1024,
  maxFiles: 25_000,
  maxFileBytes: 128 * 1024 * 1024,
  maxUncompressedBytes: 512 * 1024 * 1024,
  maxCompressionRatio: 1_000,
  maxPathLength: 220,
  maxDepth: 32,
};

export class WebArchiveError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status = 400,
  ) {
    super(message);
    this.name = "WebArchiveError";
  }
}

export interface ExtractedArchive {
  fileCount: number;
  uncompressedBytes: number;
  files: string[];
}

export interface DetectedNexoRoot {
  root: string;
  relativeRoot: string;
  itemDirectory: "items" | "item";
  itemFileCount: number;
  assetFileCount: number;
  markerCount: number;
  candidates: string[];
}

interface ValidatedPath {
  path: string;
  directory: boolean;
  collisionKey: string;
}

const WINDOWS_RESERVED = /^(?:con|prn|aux|nul|clock[$]|com[1-9]|lpt[1-9])(?:[.].*)?$/i;

function validateArchivePath(rawName: string, limits: ArchiveLimits): ValidatedPath {
  if (!rawName || /[\u0000-\u001f\u007f]/.test(rawName)) {
    throw new WebArchiveError("ZIP_PATH_INVALID", "ZIP contains an empty path or control character");
  }
  if (rawName.includes("\\")) {
    throw new WebArchiveError("ZIP_BACKSLASH_REJECTED", "ZIP paths must use forward slashes: " + rawName);
  }
  if (rawName.startsWith("/") || /^[A-Za-z]:/.test(rawName)) {
    throw new WebArchiveError("ZIP_PATH_ABSOLUTE", "ZIP contains an absolute path: " + rawName);
  }
  const directory = rawName.endsWith("/");
  const normalized = directory ? rawName.slice(0, -1) : rawName;
  if (!normalized || normalized !== normalized.normalize("NFC") || Buffer.byteLength(normalized, "utf8") > limits.maxPathLength || normalized.includes(":")) {
    throw new WebArchiveError("ZIP_PATH_INVALID", "ZIP path is invalid or too long: " + rawName);
  }
  const segments = normalized.split("/");
  if (segments.length > limits.maxDepth) {
    throw new WebArchiveError("ZIP_DEPTH_LIMIT", "ZIP path exceeds the maximum directory depth: " + rawName, 413);
  }
  for (const segment of segments) {
    if (!segment || segment === "." || segment === ".." || Buffer.byteLength(segment, "utf8") > 120 || /[. ]$/.test(segment) || WINDOWS_RESERVED.test(segment)) {
      throw new WebArchiveError("ZIP_PATH_UNSAFE", "ZIP contains an unsafe path segment: " + rawName);
    }
  }
  return {
    path: segments.join("/"),
    directory,
    collisionKey: segments.join("/").normalize("NFC").toLowerCase(),
  };
}

function isIgnoredSystemFile(path: string): boolean {
  return path === ".DS_Store" || path.endsWith("/.DS_Store") || path.startsWith("__MACOSX/");
}

function assertZipMagic(data: Uint8Array): void {
  if (data.length < 4 || data[0] !== 0x50 || data[1] !== 0x4b || !(
    (data[2] === 0x03 && data[3] === 0x04) ||
    (data[2] === 0x05 && data[3] === 0x06) ||
    (data[2] === 0x07 && data[3] === 0x08)
  )) {
    throw new WebArchiveError("ZIP_SIGNATURE_INVALID", "The uploaded file is not a valid ZIP archive");
  }
}

export async function extractZipArchive(
  data: Uint8Array,
  destination: string,
  limits: ArchiveLimits = DEFAULT_ARCHIVE_LIMITS,
): Promise<ExtractedArchive> {
  if (data.byteLength > limits.maxUploadBytes) {
    throw new WebArchiveError("UPLOAD_TOO_LARGE", "ZIP exceeds the configured upload limit", 413);
  }
  assertZipMagic(data);
  await mkdir(destination, { recursive: true, mode: 0o700 });

  const validatedByRaw = new Map<string, ValidatedPath>();
  const collisionKeys = new Set<string>();
  const filePaths = new Set<string>();
  const directoryPrefixes = new Set<string>();
  let fileCount = 0;
  let declaredBytes = 0;
  let files: Record<string, Uint8Array>;
  try {
    files = unzipSync(data, {
      filter(info: UnzipFileInfo): boolean {
        const validated = validateArchivePath(info.name, limits);
        if (validated.directory || isIgnoredSystemFile(validated.path)) return false;
        if (collisionKeys.has(validated.collisionKey) || directoryPrefixes.has(validated.collisionKey)) {
          throw new WebArchiveError("ZIP_PATH_COLLISION", "ZIP contains duplicate, case-colliding, or file/directory-conflicting paths: " + info.name);
        }
        const segments = validated.collisionKey.split("/");
        for (let index = 1; index < segments.length; index++) {
          const prefix = segments.slice(0, index).join("/");
          if (filePaths.has(prefix)) {
            throw new WebArchiveError("ZIP_PATH_COLLISION", "ZIP contains a file/directory prefix conflict: " + info.name);
          }
          directoryPrefixes.add(prefix);
        }
        collisionKeys.add(validated.collisionKey);
        filePaths.add(validated.collisionKey);
        validatedByRaw.set(info.name, validated);
        fileCount++;
        declaredBytes += info.originalSize;
        if (fileCount > limits.maxFiles) {
          throw new WebArchiveError("ZIP_FILE_LIMIT", "ZIP contains too many files", 413);
        }
        if (info.originalSize > limits.maxFileBytes) {
          throw new WebArchiveError("ZIP_ENTRY_TOO_LARGE", "ZIP entry is too large: " + info.name, 413);
        }
        if (declaredBytes > limits.maxUncompressedBytes) {
          throw new WebArchiveError("ZIP_EXPANDED_LIMIT", "ZIP expands beyond the configured size limit", 413);
        }
        if (info.size > 0 && info.originalSize > 1024 * 1024 && info.originalSize / info.size > limits.maxCompressionRatio) {
          throw new WebArchiveError("ZIP_RATIO_LIMIT", "ZIP entry has a suspicious compression ratio: " + info.name, 413);
        }
        if (info.compression !== 0 && info.compression !== 8) {
          throw new WebArchiveError("ZIP_COMPRESSION_UNSUPPORTED", "ZIP uses an unsupported compression method: " + info.name);
        }
        return true;
      },
    });
  } catch (error) {
    if (error instanceof WebArchiveError) throw error;
    throw new WebArchiveError("ZIP_INVALID", "Unable to read ZIP archive: " + String(error));
  }

  const outputRoot = resolve(destination);
  const written: string[] = [];
  let actualBytes = 0;
  for (const [rawName, bytes] of Object.entries(files)) {
    const validated = validatedByRaw.get(rawName) ?? validateArchivePath(rawName, limits);
    actualBytes += bytes.byteLength;
    if (bytes.byteLength > limits.maxFileBytes || actualBytes > limits.maxUncompressedBytes) {
      throw new WebArchiveError("ZIP_EXPANDED_LIMIT", "ZIP exceeded its declared expansion size while extracting", 413);
    }
    const target = resolve(outputRoot, ...validated.path.split("/"));
    if (!target.startsWith(outputRoot + sep)) {
      throw new WebArchiveError("ZIP_SLIP_BLOCKED", "ZIP entry escapes the extraction directory: " + rawName);
    }
    await mkdir(dirname(target), { recursive: true, mode: 0o700 });
    await writeFile(target, Buffer.from(bytes), { flag: "wx", mode: 0o600 });
    written.push(validated.path);
  }
  if (written.length === 0) {
    throw new WebArchiveError("ZIP_EMPTY", "ZIP does not contain any usable files");
  }
  return { fileCount: written.length, uncompressedBytes: actualBytes, files: written.sort() };
}

interface Candidate {
  relativeRoot: string;
  itemDirectory: "items" | "item";
  itemFiles: number;
  assets: number;
  markers: number;
  score: number;
}

async function listRegularFiles(root: string): Promise<string[]> {
  const files: string[] = [];
  const visit = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      const absolute = resolve(directory, entry.name);
      if (entry.isSymbolicLink()) throw new WebArchiveError("FILESYSTEM_LINK_REJECTED", "Symbolic links are not allowed in conversion workspaces: " + relative(root, absolute), 500);
      if (entry.isDirectory()) await visit(absolute);
      else if (entry.isFile()) files.push(relative(root, absolute).replaceAll("\\", "/"));
    }
  };
  await visit(root);
  return files;
}

export async function detectNexoRoot(extractionRoot: string): Promise<DetectedNexoRoot> {
  const root = resolve(extractionRoot);
  const files = await listRegularFiles(root);
  const groups = new Map<string, { itemDirectory: "items" | "item"; itemFiles: number }>();
  for (const path of files) {
    const segments = path.split("/");
    for (let index = 0; index < segments.length - 1; index++) {
      const segment = segments[index]!.toLowerCase();
      if ((segment === "items" || segment === "item") && path.toLowerCase().endsWith(".yml")) {
        const relativeRoot = segments.slice(0, index).join("/") || ".";
        const current = groups.get(relativeRoot) ?? { itemDirectory: segment as "items" | "item", itemFiles: 0 };
        current.itemFiles++;
        groups.set(relativeRoot, current);
        break;
      }
    }
  }

  const candidates: Candidate[] = [];
  for (const [relativeRoot, group] of groups) {
    const prefix = relativeRoot === "." ? "" : relativeRoot + "/";
    const rootFiles = files.filter((path) => path.startsWith(prefix)).map((path) => path.slice(prefix.length));
    const markerNames = new Set(rootFiles.filter((path) => !path.includes("/")).map((path) => path.toLowerCase()));
    const markerCount = ["settings.yml", "mechanics.yml", "sounds.yml", "languages.yml"].filter((name) => markerNames.has(name)).length;
    const assetFileCount = rootFiles.filter((path) => {
      const lower = path.toLowerCase();
      return lower.startsWith("pack/assets/") || lower.startsWith("resourcepack/assets/") || lower.startsWith("assets/");
    }).length;
    const recipeMarker = rootFiles.some((path) => path.toLowerCase().startsWith("recipes/")) ? 1 : 0;
    const glyphMarker = rootFiles.some((path) => path.toLowerCase().startsWith("glyphs/")) ? 1 : 0;
    const folderName = relativeRoot === "." ? "" : basename(relativeRoot).toLowerCase();
    const score = group.itemFiles * 2 + markerCount * 8 + Math.min(assetFileCount, 20) + recipeMarker * 3 + glyphMarker * 3 + (folderName === "nexo" ? 100 : 0);
    candidates.push({ relativeRoot, itemDirectory: group.itemDirectory, itemFiles: group.itemFiles, assets: assetFileCount, markers: markerCount + recipeMarker + glyphMarker, score });
  }
  candidates.sort((a, b) => b.score - a.score || b.itemFiles - a.itemFiles || a.relativeRoot.split("/").length - b.relativeRoot.split("/").length || a.relativeRoot.localeCompare(b.relativeRoot));
  if (candidates.length === 0) {
    throw new WebArchiveError("NEXO_ROOT_NOT_FOUND", "No Nexo items/ or item/ directory containing .yml files was found", 422);
  }
  const explicitNexo = candidates.filter((entry) => entry.relativeRoot !== "." && basename(entry.relativeRoot).toLowerCase() === "nexo");
  if (explicitNexo.length > 1) {
    throw new WebArchiveError("NEXO_ROOT_AMBIGUOUS", "Multiple explicit Nexo roots were found: " + explicitNexo.slice(0, 5).map((entry) => entry.relativeRoot).join(", "), 422);
  }
  if (explicitNexo.length === 0 && candidates.length > 1) {
    throw new WebArchiveError("NEXO_ROOT_AMBIGUOUS", "Multiple possible roots were found and none is named Nexo: " + candidates.slice(0, 5).map((entry) => entry.relativeRoot).join(", "), 422);
  }
  const selected = explicitNexo[0] ?? candidates[0]!;
  return {
    root: selected.relativeRoot === "." ? root : resolve(root, ...selected.relativeRoot.split("/")),
    relativeRoot: selected.relativeRoot,
    itemDirectory: selected.itemDirectory,
    itemFileCount: selected.itemFiles,
    assetFileCount: selected.assets,
    markerCount: selected.markers,
    candidates: candidates.map((entry) => entry.relativeRoot),
  };
}

export async function zipDirectory(
  directory: string,
  limits: ArchiveLimits = DEFAULT_ARCHIVE_LIMITS,
  archivePrefix = "",
): Promise<Uint8Array> {
  const root = resolve(directory);
  const files = await listRegularFiles(root);
  if (files.length > limits.maxFiles) {
    throw new WebArchiveError("OUTPUT_FILE_LIMIT", "Converted output contains too many files", 500);
  }
  const archive: Zippable = {};
  let total = 0;
  for (const path of files.sort()) {
    const bytes = await readFile(resolve(root, ...path.split("/")));
    total += bytes.byteLength;
    if (bytes.byteLength > limits.maxFileBytes || total > limits.maxUncompressedBytes) {
      throw new WebArchiveError("OUTPUT_SIZE_LIMIT", "Converted output exceeds the configured ZIP size limit", 500);
    }
    const archivePath = validateArchivePath(archivePrefix ? archivePrefix.replace(/\/$/, "") + "/" + path : path, limits).path;
    archive[archivePath] = new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  if (files.length === 0) throw new WebArchiveError("OUTPUT_EMPTY", "Converter did not produce any output files", 500);
  try {
    return zipSync(archive, { level: 6 });
  } catch (error) {
    throw new WebArchiveError("OUTPUT_ZIP_FAILED", "Unable to create CraftEngine ZIP: " + String(error), 500);
  }
}
