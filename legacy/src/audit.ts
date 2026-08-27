import { access, readFile, readdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { DiagnosticBag } from "./diagnostics.js";
import { assetFile, normalizeModelLocation, normalizeTextureLocation, splitLocation } from "./resource-location.js";
import { isObject, type JsonObject, type JsonValue } from "./types.js";

export interface AuditInput {
  resourceRoot: string;
  items: JsonObject;
  blocks: JsonObject;
  images?: JsonObject;
  blueprintRoot?: string;
}

export interface AuditSummary {
  referencedModels: number;
  resolvedModels: number;
  generatedModels: number;
  referencedBlueprints: number;
  missingBlueprints: number;
  copiedItemDefinitions: number;
  referencedTextures: number;
  resolvedTextures: number;
  missingModels: number;
  missingTextures: number;
}

async function exists(file: string): Promise<boolean> {
  try { await access(file); return true; } catch { return false; }
}

async function listJsonFiles(directory: string): Promise<string[]> {
  if (!(await exists(directory))) return [];
  const files: string[] = [];
  const visit = async (path: string): Promise<void> => {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      const child = join(path, entry.name);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile() && entry.name.toLowerCase().endsWith(".json")) files.push(child);
    }
  };
  await visit(directory);
  return files;
}

function knownVanillaParent(location: string): boolean {
  if (location === "builtin:entity" || location.startsWith("builtin:")) return true;
  if (!location.startsWith("minecraft:")) return false;
  const path = location.slice("minecraft:".length);
  return path === "item/generated" || path === "item/handheld" || path === "item/handheld_rod" || path.startsWith("item/template_") ||
    path === "block/block" || path === "block/cube" || path.startsWith("block/cube_") || path.startsWith("block/orientable") ||
    path === "block/cross" || path.startsWith("block/template_") || path.includes("stairs") || path.includes("slab");
}

function addModel(refs: Map<string, boolean>, raw: string, generated: boolean): void {
  refs.set(raw, (refs.get(raw) ?? false) || generated);
}

function collectGeneration(generation: JsonObject, modelRefs: Map<string, boolean>, textureRefs: Set<string>): void {
  if (typeof generation.parent === "string") addModel(modelRefs, generation.parent, false);
  if (isObject(generation.textures)) for (const texture of Object.values(generation.textures)) {
    if (typeof texture === "string" && !texture.startsWith("#")) textureRefs.add(texture);
  }
}

function collectModelNodes(value: JsonValue, modelRefs: Map<string, boolean>, textureRefs: Set<string>, blueprintRefs: Set<string>, parentKey = ""): void {
  if (Array.isArray(value)) {
    for (const entry of value) collectModelNodes(entry, modelRefs, textureRefs, blueprintRefs, parentKey);
    return;
  }
  if (!isObject(value)) {
    if (typeof value === "string" && parentKey === "model") addModel(modelRefs, value, false);
    return;
  }
  const type = typeof value.type === "string" ? value.type.replace(/^minecraft:/, "") : "";
  const generation = isObject(value.generation) ? value.generation : undefined;
  if (type === "model") {
    const path = typeof value.path === "string" ? value.path : typeof value.model === "string" ? value.model : undefined;
    if (path) addModel(modelRefs, path, Boolean(generation) || typeof value.blueprint === "string");
  } else if (type === "special" && typeof value.base === "string") {
    addModel(modelRefs, value.base, Boolean(generation) || typeof value.blueprint === "string");
  } else if (typeof value.path === "string" && ("predicate" in value || generation)) {
    addModel(modelRefs, value.path, Boolean(generation) || typeof value.blueprint === "string");
  }
  if (generation) collectGeneration(generation, modelRefs, textureRefs);
  if (typeof value.blueprint === "string") blueprintRefs.add(value.blueprint);
  for (const [key, entry] of Object.entries(value)) collectModelNodes(entry, modelRefs, textureRefs, blueprintRefs, key);
}

async function readObject(file: string, diagnostics: DiagnosticBag, code: string): Promise<JsonObject | undefined> {
  try {
    const raw = JSON.parse((await readFile(file, "utf8")).replace(/^\uFEFF/, "")) as JsonValue;
    if (!isObject(raw)) throw new Error("JSON root is not an object");
    return raw;
  } catch (error) {
    diagnostics.error(code, String(error), { source: file });
    return undefined;
  }
}

export async function auditResourceGraph(input: AuditInput, diagnostics: DiagnosticBag): Promise<AuditSummary> {
  const modelRefs = new Map<string, boolean>();
  const textureRefs = new Set<string>();
  const blueprintRefs = new Set<string>();
  const generatedPointers = new Set<string>();
  for (const item of Object.values(input.items)) if (isObject(item)) {
    if (item.model !== undefined) collectModelNodes(item.model, modelRefs, textureRefs, blueprintRefs);
    if (item.legacy_model !== undefined) collectModelNodes(item.legacy_model, modelRefs, textureRefs, blueprintRefs);
    if (typeof item.item_model === "string" && item.model !== undefined) generatedPointers.add(item.item_model);
  }
  for (const block of Object.values(input.blocks)) if (isObject(block)) collectModelNodes(block, modelRefs, textureRefs, blueprintRefs);
  for (const image of Object.values(input.images ?? {})) if (isObject(image) && typeof image.file === "string") textureRefs.add(image.file);

  let missingBlueprints = 0;
  const blueprintRoot = input.blueprintRoot ?? join(dirname(input.resourceRoot), "blueprint");
  for (const blueprint of blueprintRefs) {
    const file = join(blueprintRoot, (blueprint.endsWith(".bbmodel") ? blueprint : blueprint + ".bbmodel").replaceAll("/", "\\"));
    if (!(await exists(file))) {
      diagnostics.error("BLUEPRINT_FILE_MISSING", "Referenced Blockbench blueprint does not exist: " + blueprint, { source: file, lossy: true });
      missingBlueprints++;
    }
  }
  let copiedItemDefinitions = 0;
  const assetsRoot = join(input.resourceRoot, "assets");
  for (const file of await listJsonFiles(assetsRoot)) {
    const normalized = file.replaceAll("\\", "/");
    if (!normalized.includes("/items/")) continue;
    copiedItemDefinitions++;
    const definition = await readObject(file, diagnostics, "ITEM_DEFINITION_JSON_INVALID");
    if (definition?.model !== undefined) collectModelNodes(definition.model, modelRefs, textureRefs, blueprintRefs);
  }
  for (const pointer of generatedPointers) {
    const [namespace, path] = splitLocation(pointer.includes(":") ? pointer : "minecraft:" + pointer);
    const file = join(input.resourceRoot, "assets", namespace, "items", path + ".json");
    if (await exists(file)) diagnostics.warning("ITEM_DEFINITION_CONFLICT", "A copied item definition occupies a path CraftEngine will generate: " + pointer, { source: file, lossy: true });
  }

  const visitedModels = new Set<string>();
  let generatedModels = 0;
  let missingModels = 0;
  let missingTextures = 0;
  const visitModel = async (rawLocation: string, generated: boolean, source: string): Promise<void> => {
    if (rawLocation.startsWith("builtin/")) return;
    const location = rawLocation.startsWith("builtin:") ? rawLocation : normalizeModelLocation(rawLocation, diagnostics, { source, field: "resource graph" });
    if (!location || visitedModels.has(location)) return;
    visitedModels.add(location);
    if (generated) {
      generatedModels++;
      return;
    }
    if (knownVanillaParent(location)) return;
    const file = assetFile(input.resourceRoot, "models", location, ".json");
    if (!(await exists(file))) {
      diagnostics.error("MODEL_FILE_MISSING", "Referenced static model does not exist: " + location, { source, field: file, lossy: true });
      missingModels++;
      return;
    }
    const model = await readObject(file, diagnostics, "MODEL_JSON_INVALID");
    if (!model) return;
    if (typeof model.parent === "string") await visitModel(model.parent, false, file);
    if (isObject(model.textures)) for (const texture of Object.values(model.textures)) {
      if (typeof texture !== "string" || texture.startsWith("#")) continue;
      textureRefs.add(texture);
    }
    if (Array.isArray(model.overrides)) for (const override of model.overrides) {
      if (isObject(override) && typeof override.model === "string") await visitModel(override.model, false, file);
    }
  };

  for (const [location, generated] of modelRefs) await visitModel(location, generated, "configuration");
  const normalizedTextures = new Set<string>();
  for (const texture of textureRefs) {
    const location = normalizeTextureLocation(texture, diagnostics, { source: "resource graph", field: "texture" });
    if (!location || location === "minecraft:missingno" || normalizedTextures.has(location)) continue;
    normalizedTextures.add(location);
    const file = assetFile(input.resourceRoot, "textures", location, ".png");
    if (!(await exists(file))) {
      diagnostics.error("TEXTURE_FILE_MISSING", "Referenced texture does not exist: " + location, { source: file, lossy: true });
      missingTextures++;
    }
  }
  return {
    referencedModels: modelRefs.size,
    resolvedModels: visitedModels.size - missingModels,
    generatedModels,
    referencedBlueprints: blueprintRefs.size,
    missingBlueprints,
    copiedItemDefinitions,
    referencedTextures: normalizedTextures.size,
    resolvedTextures: normalizedTextures.size - missingTextures,
    missingModels,
    missingTextures,
  };
}
