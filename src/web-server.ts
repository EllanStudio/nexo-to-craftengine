#!/usr/bin/env node
import { spawn } from "node:child_process";
import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { convert, type ConvertOptions, type ConversionResult } from "./converter.js";
import { inferAuthorNamespaceFromBundlePaths } from "./source-namespace.js";
import {
  DEFAULT_ARCHIVE_LIMITS,
  WebArchiveError,
  detectNexoRoot,
  extractZipArchive,
  zipDirectory,
  type ArchiveLimits,
  type DetectedNexoRoot,
  type ExtractedArchive,
} from "./web-archive.js";

const MODULE_DIRECTORY = dirname(fileURLToPath(import.meta.url));
const DEFAULT_WEB_ROOT = resolve(MODULE_DIRECTORY, "../../web");
const DEFAULT_VENDOR_FILE = resolve(MODULE_DIRECTORY, "../../node_modules/fflate/umd/index.js");
const LOOPBACK_HOST = "127.0.0.1";

export interface WebServerOptions {
  port?: number;
  open?: boolean;
  token?: string;
  webRoot?: string;
  vendorFile?: string;
  tempRoot?: string;
  limits?: Partial<ArchiveLimits>;
}

export interface LocalWebServer {
  server: Server;
  url: string;
  token: string;
  port: number;
  close(): Promise<void>;
}

interface StaticAsset {
  bytes: Buffer;
  contentType: string;
}

interface ProblemDetails {
  status: number;
  code: string;
  title: string;
  detail: string;
  stage: string;
}

class WebRequestError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly stage = "request",
  ) {
    super(message);
    this.name = "WebRequestError";
  }
}

interface ParsedWebOptions {
  clientMode: ConvertOptions["clientMode"];
  cmdPolicy: ConvertOptions["cmdPolicy"];
  strict: boolean;
  audit: boolean;
}

interface ConversionMetadata {
  requestId: string;
  success: boolean;
  detectedRoot: string;
  source: {
    zipBytes: number;
    extractedFiles: number;
    extractedBytes: number;
    itemFiles: number;
    assetFiles: number;
  };
  output: {
    items: number;
    categories: number;
    templates: number;
    furniture: number;
    blocks: number;
    recipes: number;
    sounds: number;
    glyphs: number;
    resources: number;
  };
  diagnostics: Record<string, number>;
  audit?: ConversionResult["audit"];
  options: ParsedWebOptions & { namespace: string; namespaceMode: ConversionResult["namespaceMode"] };
  elapsedMs: number;
}

function securityHeaders(response: ServerResponse): void {
  response.setHeader("Cache-Control", "no-store");
  response.setHeader("X-Content-Type-Options", "nosniff");
  response.setHeader("X-Frame-Options", "DENY");
  response.setHeader("Referrer-Policy", "no-referrer");
  response.setHeader("Cross-Origin-Resource-Policy", "same-origin");
  response.setHeader("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'");
}

function sendJson(response: ServerResponse, status: number, value: unknown, contentType = "application/json; charset=utf-8"): void {
  const bytes = Buffer.from(JSON.stringify(value));
  securityHeaders(response);
  response.writeHead(status, { "Content-Type": contentType, "Content-Length": bytes.byteLength });
  response.end(bytes);
}

function safeDetail(value: unknown): string {
  return String(value).replace(/[\r\n\t]+/g, " ").slice(0, 1_000);
}

function problemFrom(error: unknown): ProblemDetails {
  if (error instanceof WebArchiveError) {
    return {
      status: error.status,
      code: error.code,
      title: error.status === 413 ? "Archive limit exceeded" : error.status === 422 ? "Nexo package not detected" : "Archive rejected",
      detail: safeDetail(error.message),
      stage: error.code.startsWith("NEXO_") ? "detection" : error.code.startsWith("OUTPUT_") ? "packaging" : "extraction",
    };
  }
  if (error instanceof WebRequestError) {
    return { status: error.status, code: error.code, title: "Request rejected", detail: safeDetail(error.message), stage: error.stage };
  }
  return { status: 500, code: "INTERNAL_ERROR", title: "Conversion failed", detail: "The local converter encountered an unexpected error. See the terminal for details.", stage: "conversion" };
}

function sendProblem(response: ServerResponse, error: unknown, requestId: string): void {
  const problem = problemFrom(error);
  if (!response.headersSent) {
    sendJson(response, problem.status, {
      type: "about:blank",
      title: problem.title,
      status: problem.status,
      code: problem.code,
      detail: problem.detail,
      requestId,
      stage: problem.stage,
    }, "application/problem+json; charset=utf-8");
  } else if (!response.writableEnded) {
    response.destroy();
  }
}

function parseBoolean(value: string | null, fallback: boolean, name: string): boolean {
  if (value === null) return fallback;
  if (value === "true" || value === "1") return true;
  if (value === "false" || value === "0") return false;
  throw new WebRequestError(400, "OPTION_INVALID", "Invalid boolean value for " + name);
}

function oneQueryValue(url: URL, name: string): string | null {
  const values = url.searchParams.getAll(name);
  if (values.length > 1) throw new WebRequestError(400, "OPTION_DUPLICATE", "Query option appears more than once: " + name);
  return values[0] ?? null;
}

function parseOptions(url: URL): ParsedWebOptions {
  const allowed = new Set(["token", "clientMode", "cmdPolicy", "strict", "audit"]);
  for (const key of url.searchParams.keys()) {
    if (!allowed.has(key)) throw new WebRequestError(400, "OPTION_UNKNOWN", "Unknown query option: " + key);
  }
  const clientMode = oneQueryValue(url, "clientMode") ?? "hybrid";
  if (clientMode !== "modern" && clientMode !== "hybrid" && clientMode !== "legacy") {
    throw new WebRequestError(400, "CLIENT_MODE_INVALID", "clientMode must be modern, hybrid, or legacy");
  }
  const cmdPolicy = oneQueryValue(url, "cmdPolicy") ?? "preserve";
  if (cmdPolicy !== "preserve" && cmdPolicy !== "allocate" && cmdPolicy !== "omit") {
    throw new WebRequestError(400, "CMD_POLICY_INVALID", "cmdPolicy must be preserve, allocate, or omit");
  }
  return {
    clientMode,
    cmdPolicy,
    strict: parseBoolean(oneQueryValue(url, "strict"), false, "strict"),
    audit: parseBoolean(oneQueryValue(url, "audit"), true, "audit"),
  };
}

function tokenMatches(actual: string | null, expected: string): boolean {
  if (actual === null) return false;
  const a = Buffer.from(actual);
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

function assertApiAdmission(request: IncomingMessage, url: URL, token: string, expectedOrigin: string, expectedHost: string): void {
  const remote = request.socket.remoteAddress;
  if (remote !== LOOPBACK_HOST && remote !== "::ffff:" + LOOPBACK_HOST) {
    throw new WebRequestError(403, "LOOPBACK_REQUIRED", "The Web converter only accepts loopback connections");
  }
  if (request.headers.host !== expectedHost) {
    throw new WebRequestError(403, "HOST_REJECTED", "Unexpected Host header");
  }
  const origin = request.headers.origin;
  if (origin !== undefined && origin !== expectedOrigin) {
    throw new WebRequestError(403, "ORIGIN_REJECTED", "Cross-origin conversion requests are not allowed");
  }
  const fetchSite = request.headers["sec-fetch-site"];
  if (typeof fetchSite === "string" && fetchSite !== "same-origin" && fetchSite !== "none") {
    throw new WebRequestError(403, "FETCH_SITE_REJECTED", "Cross-site conversion requests are not allowed");
  }
  if (!tokenMatches(oneQueryValue(url, "token"), token)) {
    throw new WebRequestError(401, "TOKEN_INVALID", "The local Web session token is missing or invalid");
  }
}

function readRequestBody(request: IncomingMessage, maxBytes: number): Promise<Uint8Array> {
  const declared = request.headers["content-length"];
  if (declared !== undefined) {
    if (!/^[0-9]+$/.test(declared)) throw new WebRequestError(400, "CONTENT_LENGTH_INVALID", "Content-Length is invalid");
    if (Number(declared) > maxBytes) throw new WebRequestError(413, "UPLOAD_TOO_LARGE", "ZIP exceeds the configured upload limit", "upload");
  }
  return new Promise((resolveBody, rejectBody) => {
    const chunks: Buffer[] = [];
    let total = 0;
    let settled = false;
    const fail = (error: Error): void => {
      if (settled) return;
      settled = true;
      chunks.length = 0;
      rejectBody(error);
    };
    request.on("data", (chunk: Buffer) => {
      total += chunk.byteLength;
      if (total > maxBytes) {
        fail(new WebRequestError(413, "UPLOAD_TOO_LARGE", "ZIP exceeds the configured upload limit", "upload"));
        return;
      }
      if (!settled) chunks.push(chunk);
    });
    request.on("end", () => {
      if (settled) return;
      settled = true;
      resolveBody(new Uint8Array(Buffer.concat(chunks, total)));
    });
    request.on("aborted", () => fail(new WebRequestError(400, "UPLOAD_ABORTED", "Upload was aborted", "upload")));
    request.on("error", (error) => fail(new WebRequestError(400, "UPLOAD_FAILED", safeDetail(error), "upload")));
  });
}

function isInside(parent: string, child: string): boolean {
  const value = relative(parent, child);
  return value === "" || (!value.startsWith(".." + sep) && value !== ".." && !isAbsolute(value));
}

function logicalPath(path: unknown, inputRoot: string, outputRoot: string): unknown {
  if (typeof path !== "string" || !isAbsolute(path)) return path;
  const absolute = resolve(path);
  if (isInside(inputRoot, absolute)) return "input/" + relative(inputRoot, absolute).replaceAll("\\", "/");
  if (isInside(outputRoot, absolute)) return "output/" + relative(outputRoot, absolute).replaceAll("\\", "/");
  return "<local-path>/" + basename(absolute);
}

function scrubReportPaths(value: unknown, inputRoot: string, outputRoot: string): unknown {
  if (typeof value === "string") {
    return value
      .replaceAll(inputRoot, "input")
      .replaceAll(inputRoot.replaceAll("\\", "/"), "input")
      .replaceAll(outputRoot, "output")
      .replaceAll(outputRoot.replaceAll("\\", "/"), "output");
  }
  if (Array.isArray(value)) return value.map((entry) => scrubReportPaths(entry, inputRoot, outputRoot));
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, scrubReportPaths(entry, inputRoot, outputRoot)]));
  }
  return value;
}

async function sanitizeReport(
  reportFile: string,
  inputRoot: string,
  outputRoot: string,
  detected: DetectedNexoRoot,
  extracted: ExtractedArchive,
  requestId: string,
): Promise<Record<string, unknown>> {
  const raw = JSON.parse(await readFile(reportFile, "utf8")) as Record<string, unknown>;
  const parsed = scrubReportPaths(raw, inputRoot, outputRoot) as Record<string, unknown>;
  parsed.input = detected.relativeRoot;
  parsed.output = ".";
  if (parsed.options && typeof parsed.options === "object" && !Array.isArray(parsed.options)) {
    const options = parsed.options as Record<string, unknown>;
    options.input = detected.relativeRoot;
    options.output = ".";
  }
  if (Array.isArray(parsed.diagnostics)) {
    parsed.diagnostics = parsed.diagnostics.map((entry) => {
      if (!entry || typeof entry !== "object" || Array.isArray(entry)) return entry;
      const diagnostic = { ...(entry as Record<string, unknown>) };
      diagnostic.source = logicalPath(diagnostic.source, inputRoot, outputRoot);
      return diagnostic;
    });
  }
  parsed.web = {
    requestId,
    detectedRoot: detected.relativeRoot,
    extractedFiles: extracted.fileCount,
    extractedBytes: extracted.uncompressedBytes,
  };
  await writeFile(reportFile, JSON.stringify(parsed, null, 2) + "\n", "utf8");
  return parsed;
}

function conversionMetadata(
  requestId: string,
  options: ParsedWebOptions,
  result: ConversionResult,
  detected: DetectedNexoRoot,
  extracted: ExtractedArchive,
  uploadBytes: number,
  elapsedMs: number,
): ConversionMetadata {
  return {
    requestId,
    success: result.success,
    detectedRoot: detected.relativeRoot,
    source: {
      zipBytes: uploadBytes,
      extractedFiles: extracted.fileCount,
      extractedBytes: extracted.uncompressedBytes,
      itemFiles: detected.itemFileCount,
      assetFiles: detected.assetFileCount,
    },
    output: {
      items: result.itemCount,
      categories: result.categoryCount,
      templates: result.templateCount,
      furniture: result.furnitureCount,
      blocks: result.blockCount,
      recipes: result.recipeCount,
      sounds: result.soundCount,
      glyphs: result.glyphCount,
      resources: result.resourceCount,
    },
    diagnostics: result.diagnostics.counts(),
    audit: result.audit,
    options: { ...options, namespace: result.namespace, namespaceMode: result.namespaceMode },
    elapsedMs,
  };
}

async function loadStaticAssets(webRoot: string, vendorFile: string): Promise<Map<string, StaticAsset>> {
  const specs: Array<[string, string, string]> = [
    ["/", join(webRoot, "index.html"), "text/html; charset=utf-8"],
    ["/app.js", resolve(MODULE_DIRECTORY, "web-client.js"), "text/javascript; charset=utf-8"],
    ["/style.css", join(webRoot, "style.css"), "text/css; charset=utf-8"],
    ["/vendor/fflate.js", vendorFile, "text/javascript; charset=utf-8"],
  ];
  const assets = new Map<string, StaticAsset>();
  for (const [route, file, contentType] of specs) assets.set(route, { bytes: await readFile(file), contentType });
  return assets;
}

function serveAsset(response: ServerResponse, asset: StaticAsset): void {
  securityHeaders(response);
  response.writeHead(200, { "Content-Type": asset.contentType, "Content-Length": asset.bytes.byteLength });
  response.end(asset.bytes);
}

function openBrowser(url: string): void {
  try {
    let child;
    if (process.platform === "win32") {
      const command = process.env.ComSpec ?? "cmd.exe";
      child = spawn(command, ["/d", "/s", "/c", "start \"\" \"" + url + "\""], { detached: true, stdio: "ignore", windowsHide: true });
    } else if (process.platform === "darwin") {
      child = spawn("open", [url], { detached: true, stdio: "ignore" });
    } else {
      child = spawn("xdg-open", [url], { detached: true, stdio: "ignore" });
    }
    child.unref();
  } catch (error) {
    console.warn("Unable to open the browser automatically: " + safeDetail(error));
  }
}

export async function startWebServer(options: WebServerOptions = {}): Promise<LocalWebServer> {
  const port = options.port ?? 3210;
  if (!Number.isInteger(port) || port < 0 || port > 65_535) throw new Error("Invalid Web server port");
  const token = options.token ?? randomBytes(24).toString("base64url");
  const webRoot = options.webRoot ?? DEFAULT_WEB_ROOT;
  const vendorFile = options.vendorFile ?? DEFAULT_VENDOR_FILE;
  const limits: ArchiveLimits = { ...DEFAULT_ARCHIVE_LIMITS, ...options.limits };
  const tempBase = resolve(options.tempRoot ?? tmpdir());
  await mkdir(tempBase, { recursive: true });
  const assets = await loadStaticAssets(webRoot, vendorFile);
  let active = false;
  let expectedHost = "";
  let expectedOrigin = "";

  const server = createServer(async (request, response) => {
    const requestId = randomUUID();
    try {
      if (!request.url) throw new WebRequestError(400, "URL_MISSING", "Request URL is missing");
      const url = new URL(request.url, expectedOrigin || "http://127.0.0.1");
      const route = url.pathname;

      if (request.method === "GET" && route === "/api/health") {
        sendJson(response, 200, { ok: true, converter: "nexo-to-craftengine", version: "0.1.0", limits });
        return;
      }
      if (request.method === "GET" && route === "/favicon.ico") {
        securityHeaders(response);
        response.writeHead(204);
        response.end();
        return;
      }
      if (request.method === "GET" && route === "/" && !tokenMatches(oneQueryValue(url, "token"), token)) {
        securityHeaders(response);
        response.writeHead(302, { Location: "/?token=" + encodeURIComponent(token) });
        response.end();
        return;
      }
      if (request.method === "GET") {
        const asset = assets.get(route);
        if (asset) {
          serveAsset(response, asset);
          return;
        }
      }

      if (route !== "/api/convert") throw new WebRequestError(404, "ROUTE_NOT_FOUND", "Route not found");
      if (request.method !== "POST") throw new WebRequestError(405, "METHOD_NOT_ALLOWED", "Use POST for conversion");
      assertApiAdmission(request, url, token, expectedOrigin, expectedHost);
      const contentType = String(request.headers["content-type"] ?? "").split(";", 1)[0]!.trim().toLowerCase();
      if (contentType !== "application/zip" && contentType !== "application/octet-stream") {
        throw new WebRequestError(415, "CONTENT_TYPE_INVALID", "Upload must use application/zip", "upload");
      }
      if (active) {
        response.setHeader("Retry-After", "2");
        throw new WebRequestError(429, "CONVERTER_BUSY", "Another conversion is already running", "admission");
      }
      active = true;
      let requestDirectory: string | undefined;
      const started = Date.now();
      try {
        const parsedOptions = parseOptions(url);
        const body = await readRequestBody(request, limits.maxUploadBytes);
        requestDirectory = await mkdtemp(join(tempBase, "nexo2ce-web-"));
        const inputDirectory = join(requestDirectory, "input");
        const outputDirectory = join(requestDirectory, "output");
        const extracted = await extractZipArchive(body, inputDirectory, limits);
        const detected = await detectNexoRoot(inputDirectory);
        const sourceNamespace = inferAuthorNamespaceFromBundlePaths(extracted.files, detected.relativeRoot);
        const result = await convert({
          input: detected.root,
          output: outputDirectory,
          sourceNamespace,
          clientMode: parsedOptions.clientMode,
          cmdPolicy: parsedOptions.cmdPolicy,
          strict: parsedOptions.strict,
          audit: parsedOptions.audit,
          force: false,
        });
        if (!result.reportFile) throw new WebRequestError(500, "REPORT_MISSING", "Converter did not create its report", "conversion");
        await sanitizeReport(result.reportFile, detected.root, outputDirectory, detected, extracted, requestId);
        const metadata = conversionMetadata(requestId, parsedOptions, result, detected, extracted, body.byteLength, Date.now() - started);
        await writeFile(join(outputDirectory, "conversion-response.json"), JSON.stringify(metadata, null, 2) + "\n", "utf8");
        // CraftEngine's documented install root is plugins/CraftEngine/resources/<pack>.
        // Keep that wrapper in Web downloads so extraction produces a valid pack
        // workspace and editor resource discovery works without manual relocation.
        const outputZip = await zipDirectory(outputDirectory, limits, "resources/" + result.namespace);
        await rm(requestDirectory, { recursive: true, force: true });
        requestDirectory = undefined;

        securityHeaders(response);
        response.setHeader("Content-Type", "application/zip");
        response.setHeader("Content-Disposition", "attachment; filename=\"craftengine-" + result.namespace + ".zip\"");
        response.setHeader("Content-Length", outputZip.byteLength);
        response.setHeader("X-Request-ID", requestId);
        response.setHeader("X-Conversion-Success", String(result.success));
        response.setHeader("X-Conversion-Items", String(result.itemCount));
        response.setHeader("X-Conversion-Categories", String(result.categoryCount));
        response.setHeader("X-Conversion-Errors", String(result.diagnostics.counts().error ?? 0));
        response.setHeader("X-Conversion-Warnings", String(result.diagnostics.counts().warning ?? 0));
        response.setHeader("X-Conversion-Namespace", result.namespace);
        response.setHeader("X-Conversion-Namespace-Mode", result.namespaceMode);
        response.end(Buffer.from(outputZip));
      } finally {
        active = false;
        if (requestDirectory) await rm(requestDirectory, { recursive: true, force: true }).catch((error) => console.warn("Temporary cleanup failed: " + safeDetail(error)));
      }
    } catch (error) {
      if (!(error instanceof WebArchiveError) && !(error instanceof WebRequestError)) console.error("[" + requestId + "]", error);
      sendProblem(response, error, requestId);
    }
  });

  server.requestTimeout = 5 * 60_000;
  server.headersTimeout = 30_000;
  server.keepAliveTimeout = 5_000;
  await new Promise<void>((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(port, LOOPBACK_HOST, () => {
      server.off("error", rejectListen);
      resolveListen();
    });
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Unable to determine local Web server address");
  expectedHost = LOOPBACK_HOST + ":" + address.port;
  expectedOrigin = "http://" + expectedHost;
  const url = expectedOrigin + "/?token=" + encodeURIComponent(token);
  if (options.open !== false) openBrowser(url);
  return {
    server,
    url,
    token,
    port: address.port,
    close: () => new Promise<void>((resolveClose, rejectClose) => server.close((error) => error ? rejectClose(error) : resolveClose())),
  };
}

const HELP = [
  "Nexo 1.26 -> CraftEngine 26.8 local Web converter",
  "",
  "Usage: node dist/src/web-server.js [--port 3210] [--no-open]",
  "",
  "The server binds only to 127.0.0.1 and uses a random session token.",
].join("\n");

async function main(): Promise<void> {
  let port = 3210;
  let shouldOpen = true;
  for (let index = 2; index < process.argv.length; index++) {
    const argument = process.argv[index]!;
    if (argument === "--help" || argument === "-h") {
      console.log(HELP);
      return;
    }
    if (argument === "--no-open") {
      shouldOpen = false;
      continue;
    }
    if (argument === "--port") {
      const raw = process.argv[++index];
      if (!raw || !/^[0-9]+$/.test(raw)) throw new Error("--port requires an integer");
      port = Number(raw);
      continue;
    }
    throw new Error("Unknown option: " + argument);
  }
  const local = await startWebServer({ port, open: shouldOpen });
  console.log("Local Web converter: " + local.url);
  console.log("Press Ctrl+C to stop.");
  const stop = async (): Promise<void> => {
    await local.close().catch(() => undefined);
    process.exit(0);
  };
  process.once("SIGINT", () => { void stop(); });
  process.once("SIGTERM", () => { void stop(); });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error: unknown) => {
    console.error("nexo2ce-web: " + safeDetail(error));
    process.exitCode = 1;
  });
}
