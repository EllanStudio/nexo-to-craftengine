import assert from "node:assert/strict";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { strFromU8, strToU8, unzipSync, zipSync } from "fflate";
import { parse } from "yaml";
import { DEFAULT_ARCHIVE_LIMITS, WebArchiveError, detectNexoRoot, extractZipArchive } from "../src/web-archive.js";
import { startWebServer } from "../src/web-server.js";
import type { JsonObject } from "../src/types.js";

function archiveBuffer(files: Record<string, string>): Uint8Array {
  return zipSync(Object.fromEntries(Object.entries(files).map(([path, value]) => [path, strToU8(value)])), { level: 1 });
}

function bodyBuffer(bytes: Uint8Array): ArrayBuffer {
  const buffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(buffer).set(bytes);
  return buffer;
}

test("secure ZIP extraction prefers an explicit Nexo root in multi-platform bundles", async () => {
  const root = await mkdtemp(join(tmpdir(), "nexo2ce-web-archive-"));
  try {
    const archive = archiveBuffer({
      "wrapper/Nexo/items/demo.yml": "demo:\n  material: PAPER\n",
      "wrapper/Nexo/settings.yml": "Glyphs: {}\n",
      "wrapper/Nexo/pack/assets/demo/models/item/demo.json": "{}",
      "wrapper/Oraxen/items/demo.yml": "demo:\n  material: PAPER\n",
    });
    const extracted = await extractZipArchive(archive, join(root, "input"));
    assert.equal(extracted.fileCount, 4);
    const detected = await detectNexoRoot(join(root, "input"));
    assert.equal(detected.relativeRoot, "wrapper/Nexo");
    assert.equal(detected.itemFileCount, 1);
    assert.equal(detected.assetFileCount, 1);
    assert.deepEqual(detected.candidates.sort(), ["wrapper/Nexo", "wrapper/Oraxen"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("ZIP extraction rejects traversal, collisions, size bombs, and ambiguous roots", async () => {
  const root = await mkdtemp(join(tmpdir(), "nexo2ce-web-reject-"));
  try {
    const traversal = archiveBuffer({ "../escape.txt": "x" });
    await assert.rejects(
      extractZipArchive(traversal, join(root, "traversal")),
      (error: unknown) => error instanceof WebArchiveError && error.code === "ZIP_PATH_UNSAFE",
    );

    const collision = archiveBuffer({ "Folder/file.txt": "a", "folder/FILE.txt": "b" });
    await assert.rejects(
      extractZipArchive(collision, join(root, "collision")),
      (error: unknown) => error instanceof WebArchiveError && error.code === "ZIP_PATH_COLLISION",
    );

    const backslash = archiveBuffer({ "folder\\..\\escape.txt": "x" });
    await assert.rejects(
      extractZipArchive(backslash, join(root, "backslash")),
      (error: unknown) => error instanceof WebArchiveError && error.code === "ZIP_BACKSLASH_REJECTED",
    );

    const reserved = archiveBuffer({ "NUL.txt": "x" });
    await assert.rejects(
      extractZipArchive(reserved, join(root, "reserved")),
      (error: unknown) => error instanceof WebArchiveError && error.code === "ZIP_PATH_UNSAFE",
    );

    const prefixConflict = archiveBuffer({ "node": "file", "node/child.txt": "child" });
    await assert.rejects(
      extractZipArchive(prefixConflict, join(root, "prefix")),
      (error: unknown) => error instanceof WebArchiveError && error.code === "ZIP_PATH_COLLISION",
    );

    const oversized = archiveBuffer({ "Nexo/items/demo.yml": "0123456789" });
    await assert.rejects(
      extractZipArchive(oversized, join(root, "oversized"), { ...DEFAULT_ARCHIVE_LIMITS, maxFileBytes: 4 }),
      (error: unknown) => error instanceof WebArchiveError && error.code === "ZIP_ENTRY_TOO_LARGE",
    );

    const noRoot = archiveBuffer({ "readme.txt": "not a Nexo package" });
    const noRootDirectory = join(root, "no-root");
    await extractZipArchive(noRoot, noRootDirectory);
    await assert.rejects(
      detectNexoRoot(noRootDirectory),
      (error: unknown) => error instanceof WebArchiveError && error.code === "NEXO_ROOT_NOT_FOUND" && error.status === 422,
    );

    const ambiguous = archiveBuffer({
      "one/items/demo.yml": "demo:\n  material: PAPER\n",
      "two/items/demo.yml": "demo:\n  material: PAPER\n",
    });
    const ambiguousRoot = join(root, "ambiguous");
    await extractZipArchive(ambiguous, ambiguousRoot);
    await assert.rejects(
      detectNexoRoot(ambiguousRoot),
      (error: unknown) => error instanceof WebArchiveError && error.code === "NEXO_ROOT_AMBIGUOUS" && error.status === 422,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("local Web API converts a ZIP, sanitizes reports, and returns a CE ZIP", async () => {
  const root = await mkdtemp(join(tmpdir(), "nexo2ce-web-api-"));
  const jobs = join(root, "jobs");
  const local = await startWebServer({ port: 0, open: false, token: "test-session-token", tempRoot: jobs });
  try {
    const page = await fetch(local.url, { redirect: "follow" });
    assert.equal(page.status, 200);
    const pageHtml = await page.text();
    assert.match(pageHtml, /选择 Nexo 来源/);
    assert.match(pageHtml, /命名空间[\s\S]*自动识别[\s\S]*<code>读取作者原包<\/code>/);
    assert.doesNotMatch(pageHtml, /id="namespaceInput"/);
    const client = await fetch(new URL("/app.js", local.url));
    assert.equal(client.status, 200);
    assert.match(pageHtml, /id="copyDiagnosticsButton"/);
    assert.match(pageHtml, /id="diagnosticSearch"/);
    assert.match(pageHtml, /id="toastContainer"/);
    const clientScript = await client.text();
    assert.match(clientScript, /runConversion/);
    assert.match(clientScript, /copyDiagnosticsButton/);
    assert.match(clientScript, /filterDiagnosticGroups/);
    assert.doesNotMatch(clientScript, /namespaceInput/);
    const health = await fetch(new URL("/api/health", local.url));
    assert.equal((await health.json() as { ok: boolean }).ok, true);

    const archive = archiveBuffer({
      "bundle/Nexo/items/demo.yml": [
        "demo:",
        "  itemname: Demo",
        "  material: PAPER",
        "  Pack:",
        "    model: custom/demo",
        "    custom_model_data: 1234",
        "  Mechanics:",
        "    furniture:",
        "      limited_placing:",
        "        floor: false",
        "        roof: false",
        "        wall: false",
        "      properties:",
        "        offset_against_blocks: false",
        "      hitbox:",
        "        interactions:",
        "          - '0,0,0 1,1'",
        "",
      ].join("\n"),
      "bundle/Nexo/pack/assets/minecraft/models/custom/demo.json": JSON.stringify({ parent: "minecraft:item/generated", textures: { layer0: "custom/demo" } }),
      "bundle/Nexo/pack/assets/minecraft/textures/custom/demo.png": "fixture",
    });
    const endpoint = new URL("/api/convert", local.url);
    endpoint.search = new URLSearchParams({
      token: "test-session-token",
      clientMode: "hybrid",
      cmdPolicy: "preserve",
      strict: "true",
      audit: "true",
    }).toString();

    const noToken = new URL(endpoint);
    noToken.searchParams.delete("token");
    const unauthorized = await fetch(noToken, { method: "POST", headers: { "Content-Type": "application/zip" }, body: bodyBuffer(archive) });
    assert.equal(unauthorized.status, 401);

    const crossOrigin = await fetch(endpoint, { method: "POST", headers: { "Content-Type": "application/zip", Origin: "http://evil.invalid" }, body: bodyBuffer(archive) });
    assert.equal(crossOrigin.status, 403);

    const manualNamespace = new URL(endpoint);
    manualNamespace.searchParams.set("namespace", "renamed");
    const manualResponse = await fetch(manualNamespace, { method: "POST", headers: { "Content-Type": "application/zip" }, body: bodyBuffer(archive) });
    assert.equal(manualResponse.status, 400);
    assert.equal((await manualResponse.json() as { code: string }).code, "OPTION_UNKNOWN");

    const response = await fetch(endpoint, { method: "POST", headers: { "Content-Type": "application/zip" }, body: bodyBuffer(archive) });
    assert.equal(response.status, 200, await response.clone().text());
    assert.equal(response.headers.get("content-type"), "application/zip");
    assert.equal(response.headers.get("x-conversion-success"), "true");
    assert.equal(response.headers.get("x-conversion-categories"), "1");
    assert.equal(response.headers.get("x-conversion-namespace"), "demo");
    assert.equal(response.headers.get("x-conversion-namespace-mode"), "author");
    const output = new Uint8Array(await response.arrayBuffer());
    const files = unzipSync(output);
    const prefix = "resources/demo/";
    assert.equal(files["pack.yml"], undefined);
    assert.ok(files[prefix + "pack.yml"]);
    assert.ok(files[prefix + "configuration/items.yml"]);
    assert.ok(files[prefix + "configuration/categories.yml"]);
    assert.ok(files[prefix + "configuration/furniture.yml"]);
    assert.equal(files[prefix + "configuration/furniture-templates.yml"], undefined);
    assert.ok(files[prefix + "resourcepack/assets/minecraft/models/custom/demo.json"]);
    assert.ok(files[prefix + "migration-mapping.yml"]);
    assert.ok(files[prefix + "conversion-response.json"]);
    const packConfig = parse(strFromU8(files[prefix + "pack.yml"]!)) as { namespace: string };
    const itemConfig = parse(strFromU8(files[prefix + "configuration/items.yml"]!)) as { items: Record<string, { item_model?: string; behavior?: { furniture?: string } }> };
    const categoryConfig = parse(strFromU8(files[prefix + "configuration/categories.yml"]!)) as { categories: Record<string, { list: string[] }> };
    const furnitureText = strFromU8(files[prefix + "configuration/furniture.yml"]!);
    const furnitureConfig = parse(furnitureText) as { furniture: Record<string, JsonObject> };
    const mappingConfig = parse(strFromU8(files[prefix + "migration-mapping.yml"]!)) as { items: Record<string, { target: string }> };
    assert.equal(packConfig.namespace, "demo");
    assert.deepEqual(Object.keys(itemConfig.items), ["demo:demo"]);
    assert.equal(itemConfig.items["demo:demo"]?.item_model, "demo:demo");
    assert.equal(itemConfig.items["demo:demo"]?.behavior?.furniture, "demo:demo");
    assert.deepEqual(categoryConfig.categories["demo:demo"]?.list, ["demo:demo"]);
    assert.deepEqual(Object.keys(furnitureConfig.furniture), ["demo:demo"]);
    assert.equal(furnitureConfig.furniture["demo:demo"]!.template, undefined);
    assert.equal((furnitureConfig.furniture["demo:demo"]!.settings as JsonObject).item, "demo:demo");
    assert.doesNotMatch(furnitureText, /_nexo2ce\/furniture\/variant-shift|__nexo2ce_|\$\{/u);
    assert.equal(mappingConfig.items.demo?.target, "demo:demo");
    const reportText = strFromU8(files[prefix + "conversion-report.json"]!);
    const report = JSON.parse(reportText) as {
      success: boolean; input: string; output: string; diagnostics: unknown[];
      options: { namespace: string; namespaceMode: string };
      identity: { sourceRuntimeNamespace: string; authorNamespace: string; targetItemNamespace: string; namespaceMode: string };
    };
    assert.equal(report.success, true);
    assert.equal(report.input, "bundle/Nexo");
    assert.equal(report.output, ".");
    assert.equal(report.options.namespace, "demo");
    assert.equal(report.options.namespaceMode, "author");
    assert.equal(report.identity.sourceRuntimeNamespace, "nexo");
    assert.equal(report.identity.authorNamespace, "demo");
    assert.equal(report.identity.targetItemNamespace, "demo");
    assert.equal(report.identity.namespaceMode, "author");
    assert.doesNotMatch(reportText, /nexo2ce-web-api-|nexo2ce-web-/);
    assert.equal(report.diagnostics.length, 0);
  } finally {
    await local.close();
    const remaining = await readdir(jobs).catch(() => [] as string[]);
    assert.deepEqual(remaining, []);
    await rm(root, { recursive: true, force: true });
  }
});
