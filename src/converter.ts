import { access, lstat, mkdir, readdir, realpath, rm, stat } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { auditResourceGraph, type AuditSummary } from "./audit.js";
import { DiagnosticBag } from "./diagnostics.js";
import { compactFurnitureDefinition, mergeFurnitureTemplates } from "./furniture-templates.js";
import { convertGlyphs, rewriteGlyphTags } from "./glyphs.js";
import { convertItem, resolveItemTemplates, matchBukkitMaterial, type ConvertedItem, type ResolvedItem, type SourceItem } from "./items.js";
import { loadYaml, writeJson, writeYaml } from "./io.js";
import { convertMechanics } from "./mechanics.js";
import { MINECRAFT_1_21_11_SOLID_BLOCK_COUNT } from "./minecraft-1.21.11.js";
import { discoverModelAliases } from "./model-aliases.js";
import { convertRecipe, type NexoRecipeType } from "./recipes.js";
import { validateNamespace } from "./resource-location.js";
import { copyResourcePack, findResourcePackRoot, writeLanguageResources } from "./resources.js";
import { convertSounds } from "./sounds.js";
import { inferAuthorNamespaceFromNexoFiles, type NamespaceInference } from "./source-namespace.js";
import { asStringList, getBoolean, getNumber, getObject, getString, getValue, isObject, type JsonObject, type JsonValue } from "./types.js";

export const NEXO_ITEM_NAMESPACE = "nexo";

export interface ConvertOptions {
  input: string;
  output: string;
  /** Explicit override; omit to use the author's namespace detected from source files. */
  namespace?: string;
  /** Trusted full-bundle inference supplied by the Web archive detector. */
  sourceNamespace?: NamespaceInference;
  clientMode: "modern" | "hybrid" | "legacy";
  cmdPolicy: "preserve" | "allocate" | "omit";
  strict: boolean;
  force: boolean;
  audit: boolean;
}

export interface ConversionResult {
  success: boolean;
  diagnostics: DiagnosticBag;
  reportFile?: string;
  itemCount: number;
  templateCount: number;
  furnitureCount: number;
  blockCount: number;
  recipeCount: number;
  soundCount: number;
  glyphCount: number;
  resourceCount: number;
  audit?: AuditSummary;
  namespace: string;
  namespaceMode: "author" | "fallback" | "override";
}

const RECIPE_TYPES: NexoRecipeType[] = ["shaped", "shapeless", "furnace", "blasting", "smoking", "campfire", "stonecutting", "brewing"];

async function exists(path: string): Promise<boolean> {
  try { await access(path); return true; } catch { return false; }
}

async function listFiles(directory: string, extension: string): Promise<string[]> {
  if (!(await exists(directory))) return [];
  const result: string[] = [];
  const visit = async (current: string): Promise<void> => {
    const entries = await readdir(current, { withFileTypes: true });
    for (const entry of entries) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) await visit(path);
      else if (entry.isFile() && entry.name.toLowerCase().endsWith(extension)) result.push(path);
    }
  };
  await visit(directory);
  return result.sort((a, b) => a.localeCompare(b, "en"));
}

async function resolveNexoRoot(input: string): Promise<string> {
  const absolute = resolve(input);
  const name = basename(absolute).toLowerCase();
  if (name === "items" || name === "item") return dirname(absolute);
  for (const candidate of [absolute, join(absolute, "Nexo"), join(absolute, "nexo")]) {
    if (await exists(join(candidate, "items")) || await exists(join(candidate, "item"))) return candidate;
  }
  return absolute;
}

function isMissingPathError(error: unknown): boolean {
  if (typeof error !== "object" || error === null || !("code" in error)) return false;
  const code = String((error as { code?: unknown }).code);
  return code === "ENOENT" || code === "ENOTDIR";
}

async function canonicalPath(path: string): Promise<string> {
  let cursor = resolve(path);
  const missing: string[] = [];
  while (true) {
    try {
      await lstat(cursor);
      break;
    } catch (error) {
      // Canonical overlap checks guard recursive deletion. Missing suffixes are
      // expected for a new output, but permission/reparse failures must abort.
      if (!isMissingPathError(error)) throw error;
      const parent = dirname(cursor);
      if (parent === cursor) throw error;
      missing.unshift(basename(cursor));
      cursor = parent;
    }
  }
  const canonicalBase = await realpath(cursor);
  return resolve(canonicalBase, ...missing);
}

function comparablePath(path: string): string {
  const absolute = resolve(path);
  return process.platform === "win32" ? absolute.toLowerCase() : absolute;
}

function containsPath(parent: string, child: string): boolean {
  const relation = relative(comparablePath(parent), comparablePath(child));
  return relation === "" || (!isAbsolute(relation) && relation !== ".." && !relation.startsWith(".." + sep));
}

async function prepareOutput(protectedInputs: string[], output: string, force: boolean): Promise<void> {
  const destination = resolve(output);
  const canonicalDestination = await canonicalPath(destination);
  for (const input of protectedInputs) {
    const source = await canonicalPath(input);
    if (containsPath(source, canonicalDestination) || containsPath(canonicalDestination, source)) {
      throw new Error("Output directory must not overlap the Nexo input or resource-pack directory: " + canonicalDestination);
    }
  }
  if (await exists(destination)) {
    const contents = await readdir(destination);
    if (contents.length > 0 && !force) throw new Error("Output directory is not empty; use --force to replace it: " + destination);
    if (force) await rm(destination, { recursive: true, force: true });
  }
  await mkdir(join(destination, "configuration"), { recursive: true });
  await mkdir(join(destination, "resourcepack"), { recursive: true });
}

async function uniqueCanonicalPaths(paths: string[]): Promise<string[]> {
  const unique = new Map<string, string>();
  for (const path of paths) {
    const canonical = await canonicalPath(path);
    const metadata = await stat(canonical, { bigint: true });
    // realpath collapses symlink/junction aliases. Device+inode additionally
    // collapses distinct hard-link names that identify the same YAML file.
    const key = metadata.ino === 0n
      ? "path:" + comparablePath(canonical)
      : "inode:" + metadata.dev + ":" + metadata.ino;
    if (!unique.has(key)) unique.set(key, path);
  }
  return [...unique.values()];
}

async function listItemConfigFiles(root: string): Promise<string[]> {
  const candidateDirectories = [join(root, "items"), join(root, "item")];
  const existingDirectories: string[] = [];
  for (const directory of candidateDirectories) if (await exists(directory)) existingDirectories.push(directory);
  const directories = await uniqueCanonicalPaths(existingDirectories);
  const candidates: string[] = [];
  for (const directory of directories) candidates.push(...await listFiles(directory, ".yml"));
  return (await uniqueCanonicalPaths(candidates)).sort((a, b) => a.localeCompare(b, "en"));
}

async function loadItems(root: string, diagnostics: DiagnosticBag): Promise<SourceItem[]> {
  const files = await listItemConfigFiles(root);
  const items: SourceItem[] = [];
  const ids = new Map<string, string>();
  for (const file of files) {
    const loaded = await loadYaml(file, diagnostics);
    if (!isObject(loaded)) {
      if (loaded !== undefined) diagnostics.error("ITEM_FILE_ROOT_INVALID", "Item YAML root must be a map", { source: file });
      continue;
    }
    for (const [id, config] of Object.entries(loaded)) {
      if (!isObject(config)) {
        diagnostics.error("ITEM_SECTION_INVALID", "Item section must be a map", { source: file, item: id });
        continue;
      }
      const previous = ids.get(id);
      if (previous) {
        diagnostics.error("DUPLICATE_ITEM_ID", "Item id is also defined in " + previous, { source: file, item: id });
        continue;
      }
      ids.set(id, file);
      items.push({ id, source: file, config, template: false });
    }
  }
  if (files.length === 0) diagnostics.error("ITEM_DIRECTORY_EMPTY", "No Nexo item YAML files found under items/ or item/", { source: root });
  return items;
}

function itemMaterial(item: ResolvedItem): string {
  return matchBukkitMaterial(getValue(item.config, "material")) ?? "paper";
}

function itemModelIdentity(item: ResolvedItem): string {
  const pack = getObject(item.config, "Pack");
  return pack ? getString(pack, "model") ?? getString(pack, "bbmodel") ?? item.id : item.id;
}

function explicitCmd(item: ResolvedItem): number | undefined {
  const pack = getObject(item.config, "Pack");
  const value = pack ? getNumber(pack, "custom_model_data") : undefined;
  return value !== undefined && Number.isInteger(value) && value > 0 ? value : undefined;
}

function allocateCustomModelData(items: ResolvedItem[], options: ConvertOptions, diagnostics: DiagnosticBag): Map<string, number> {
  const assignments = new Map<string, number>();
  const usedByMaterial = new Map<string, Map<number, string>>();
  const byModel = new Map<string, Map<string, number>>();
  const concrete = items.filter((item) => !item.template && getObject(item.config, "Pack"));
  for (const item of concrete) {
    const explicit = explicitCmd(item);
    if (explicit === undefined) continue;
    const material = itemMaterial(item);
    const model = itemModelIdentity(item);
    const used = usedByMaterial.get(material) ?? new Map<number, string>();
    const conflict = used.get(explicit);
    if (conflict && conflict !== model) diagnostics.error("CUSTOM_MODEL_DATA_CONFLICT", "CMD " + explicit + " on " + material + " is already used by model " + conflict, { source: item.source, item: item.id, field: "Pack.custom_model_data" });
    used.set(explicit, model);
    usedByMaterial.set(material, used);
    const models = byModel.get(material) ?? new Map<string, number>();
    models.set(model, explicit);
    byModel.set(material, models);
    if (options.cmdPolicy !== "omit") assignments.set(item.id, explicit);
    else diagnostics.warning("CUSTOM_MODEL_DATA_OMITTED", "Explicit Nexo custom_model_data was omitted by policy", { source: item.source, item: item.id, field: "Pack.custom_model_data", lossy: true });
  }
  if (options.cmdPolicy === "allocate") {
    for (const item of concrete) {
      if (assignments.has(item.id)) continue;
      const material = itemMaterial(item);
      const model = itemModelIdentity(item);
      const models = byModel.get(material) ?? new Map<string, number>();
      const existing = models.get(model);
      if (existing !== undefined) {
        assignments.set(item.id, existing);
        continue;
      }
      const used = usedByMaterial.get(material) ?? new Map<number, string>();
      let candidate = 1000;
      while (used.has(candidate)) candidate++;
      used.set(candidate, model);
      models.set(model, candidate);
      usedByMaterial.set(material, used);
      byModel.set(material, models);
      assignments.set(item.id, candidate);
      diagnostics.info("CUSTOM_MODEL_DATA_RECONSTRUCTED", "Reconstructed Nexo material-scoped CMD allocation: " + candidate, { source: item.source, item: item.id, field: "Pack.custom_model_data" });
    }
  } else if (options.cmdPolicy === "preserve" && options.clientMode !== "modern") {
    for (const item of concrete) if (!assignments.has(item.id)) {
      diagnostics.warning("CUSTOM_MODEL_DATA_NOT_EXPLICIT", "Nexo would allocate CMD at runtime, but preserve policy does not invent it; use --cmd-policy allocate after reviewing all source configs", { source: item.source, item: item.id, field: "Pack.custom_model_data", lossy: true });
    }
  }
  return assignments;
}

async function loadOptionalObject(file: string, diagnostics: DiagnosticBag): Promise<JsonObject | undefined> {
  if (!(await exists(file))) return undefined;
  const value = await loadYaml(file, diagnostics);
  return isObject(value) ? value : undefined;
}

async function convertRecipes(root: string, namespace: string, diagnostics: DiagnosticBag): Promise<JsonObject> {
  const output: JsonObject = {};
  for (const type of RECIPE_TYPES) {
    const directory = join(root, "recipes", type);
    for (const file of await listFiles(directory, ".yml")) {
      const loaded = await loadYaml(file, diagnostics);
      if (!isObject(loaded)) continue;
      for (const [id, section] of Object.entries(loaded)) {
        if (!isObject(section)) continue;
        const converted = convertRecipe(type, id, section, namespace, diagnostics, file);
        if (converted) output[namespace + ":" + id] = converted;
      }
    }
  }
  return output;
}

export async function convert(options: ConvertOptions): Promise<ConversionResult> {
  const diagnostics = new DiagnosticBag();
  const root = await resolveNexoRoot(options.input);
  const itemConfigFiles = await listItemConfigFiles(root);
  const inferredNamespace = options.sourceNamespace ?? inferAuthorNamespaceFromNexoFiles(root, itemConfigFiles);
  const namespace = options.namespace ?? inferredNamespace?.namespace ?? NEXO_ITEM_NAMESPACE;
  const namespaceMode = options.namespace !== undefined ? "override" : inferredNamespace ? "author" : "fallback";
  if (!validateNamespace(namespace)) throw new Error("Invalid namespace: " + namespace);
  const output = resolve(options.output);
  // Discover and canonicalize every protected source before creating output.
  // This prevents --force from deleting an ancestor of the input and prevents
  // resource copying from recursing into a destination nested under assets/.
  const resourcePackRoot = await findResourcePackRoot(root);
  const protectedSources = [
    root,
    join(root, "items"), join(root, "item"), join(root, "glyphs"),
    join(root, "settings.yml"), join(root, "mechanics.yml"),
    join(root, "sounds.yml"), join(root, "languages.yml"),
    ...RECIPE_TYPES.map((type) => join(root, "recipes", type)),
    ...(resourcePackRoot ? [resourcePackRoot, join(resourcePackRoot, "assets")] : []),
  ];
  await prepareOutput(protectedSources, output, options.force);

  const settingsRoot = await loadOptionalObject(join(root, "settings.yml"), diagnostics);
  const glyphSettings = settingsRoot ? getObject(settingsRoot, "Glyphs") : undefined;
  const defaultGlyphFont = glyphSettings ? getString(glyphSettings, "default_font") ?? namespace + ":default" : namespace + ":default";
  const defaultGlyphPermission = glyphSettings ? getString(glyphSettings, "default_permission") ?? "nexo.glyphs.<glyphid>" : "nexo.glyphs.<glyphid>";
  const glyphConversion = await convertGlyphs(root, namespace, diagnostics, defaultGlyphFont, defaultGlyphPermission);
  const mechanicsSettings = await loadOptionalObject(join(root, "mechanics.yml"), diagnostics);
  const furnitureSettings = mechanicsSettings ? getObject(mechanicsSettings, "furniture") : undefined;
  const furnitureDefaultProperties = furnitureSettings ? getObject(furnitureSettings, "default_properties") : undefined;
  const defaultRotatableOnSneak = furnitureSettings ? getBoolean(furnitureSettings, "default_rotatable_on_sneak", false) : false;
  const globalFurnitureSettings = settingsRoot ? getObject(settingsRoot, "Furniture") : undefined;
  const rawRotationGamemodes = globalFurnitureSettings ? getValue(globalFurnitureSettings, "allowed_gamemodes_for_rotation") : undefined;
  const rotationGamemodes = rawRotationGamemodes === undefined ? ["SURVIVAL", "CREATIVE"] : asStringList(rawRotationGamemodes);
  const sourceItems = await loadItems(root, diagnostics);
  const resolvedItems = resolveItemTemplates(sourceItems, diagnostics);
  const modelAliases = await discoverModelAliases(resourcePackRoot, resolvedItems, diagnostics);
  // Nexo custom NoteBlocks are backed by Bukkit NOTE_BLOCK, whose 1.21.11
  // Material.isSolid() value is true. CE exposes their custom ids at runtime,
  // so include every converted id in the wall-support predicate.
  const solidCustomBlockIds = resolvedItems
    .filter((entry) => !entry.template && getObject(getObject(entry.config, "Mechanics") ?? {}, "noteblock") !== undefined)
    .map((entry) => namespace + ":" + entry.id);
  const cmd = allocateCustomModelData(resolvedItems, options, diagnostics);
  const items: JsonObject = {};
  const furniture: JsonObject = {};
  const furnitureTemplates: JsonObject = {};
  const blocks: JsonObject = {};
  const mappings: JsonObject = {};
  let templateCount = 0;
  for (const sourceItem of resolvedItems) {
    if (sourceItem.template) {
      templateCount++;
      continue;
    }
    const rewrittenItem: ResolvedItem = {
      ...sourceItem,
      config: rewriteGlyphTags(sourceItem.config, glyphConversion.entries, diagnostics, sourceItem.source, sourceItem.id) as JsonObject,
    };
    const converted = convertItem(rewrittenItem, { namespace, clientMode: options.clientMode, modelAliases }, cmd.get(sourceItem.id), diagnostics);
    if (!converted) continue;
    const mechanics = convertMechanics(
      rewrittenItem.config, converted.targetId, converted.baseModel, diagnostics, sourceItem.source, sourceItem.id,
      furnitureDefaultProperties, { defaultRotatableOnSneak, rotationGamemodes, solidCustomBlockIds },
    );
    if (mechanics.behavior.length === 1) converted.config.behavior = mechanics.behavior[0]!;
    else if (mechanics.behavior.length > 1) converted.config.behaviors = mechanics.behavior;
    items[converted.targetId] = converted.config;
    if (mechanics.furniture) {
      const compacted = compactFurnitureDefinition(mechanics.furniture, converted.targetId);
      furniture[converted.targetId] = compacted.definition;
      mergeFurnitureTemplates(furnitureTemplates, compacted.templates);
    }
    if (mechanics.block) blocks[converted.targetId] = mechanics.block;
    mappings[sourceItem.id] = {
      target: converted.targetId,
      source: relative(root, sourceItem.source).replaceAll("\\", "/"),
      template: sourceItem.templateIds,
      semantics: { ...converted.semantics, ...mechanics.semantics },
    };
  }

  const recipes = await convertRecipes(root, namespace, diagnostics);
  const soundsRoot = await loadOptionalObject(join(root, "sounds.yml"), diagnostics);
  const sounds = soundsRoot ? convertSounds(soundsRoot, diagnostics, join(root, "sounds.yml")) : {};
  let resourceCount = 0;
  if (resourcePackRoot) resourceCount = await copyResourcePack(resourcePackRoot, join(output, "resourcepack"), diagnostics, join(output, "blueprint"));
  else diagnostics.warning("RESOURCE_PACK_NOT_FOUND", "No pack/assets, resourcepack/assets, or assets directory was found", { source: root, lossy: true });
  const languages = await loadOptionalObject(join(root, "languages.yml"), diagnostics);
  if (languages) await writeLanguageResources(languages, join(output, "resourcepack"), diagnostics, join(root, "languages.yml"));

  await writeYaml(join(output, "pack.yml"), {
    author: "nexo2ce",
    version: "1.0",
    description: "Converted from Nexo 1.26 with Minecraft semantic auditing",
    namespace,
  });
  // A CE pack does not need placeholder files for feature families it does not
  // contain. Emitting blocks: {}, recipes: {}, etc. invents source categories and
  // makes reviews misleading, so create each configuration file only when it has
  // at least one converted definition.
  if (Object.keys(items).length > 0) await writeYaml(join(output, "configuration", "items.yml"), { items });
  if (Object.keys(furnitureTemplates).length > 0) {
    await writeYaml(join(output, "configuration", "furniture-templates.yml"), { templates: furnitureTemplates });
  }
  if (Object.keys(furniture).length > 0) await writeYaml(join(output, "configuration", "furniture.yml"), { furniture });
  if (Object.keys(blocks).length > 0) await writeYaml(join(output, "configuration", "blocks.yml"), { blocks });
  if (Object.keys(recipes).length > 0) await writeYaml(join(output, "configuration", "recipes.yml"), { recipes });
  if (Object.keys(sounds).length > 0) await writeYaml(join(output, "configuration", "sounds.yml"), { sounds });
  if (Object.keys(glyphConversion.images).length > 0) await writeYaml(join(output, "configuration", "images.yml"), { images: glyphConversion.images });
  const glyphMappings = Object.fromEntries(Object.values(glyphConversion.entries)
    .filter((entry, index, values) => values.findIndex((candidate) => candidate.sourceId === entry.sourceId) === index)
    .map((entry) => [entry.sourceId, { target: entry.targetId, font: entry.font, chars: entry.chars, start_index: entry.startIndex }]));
  const migrationMapping: JsonObject = {};
  if (Object.keys(mappings).length > 0) migrationMapping.items = mappings;
  if (Object.keys(glyphMappings).length > 0) migrationMapping.glyphs = glyphMappings;
  if (Object.keys(migrationMapping).length > 0) await writeYaml(join(output, "migration-mapping.yml"), migrationMapping);

  let audit: AuditSummary | undefined;
  if (options.audit) audit = await auditResourceGraph({ resourceRoot: join(output, "resourcepack"), items, blocks, images: glyphConversion.images, blueprintRoot: join(output, "blueprint") }, diagnostics);
  const success = !diagnostics.hasErrors() && !(options.strict && diagnostics.hasLossy());
  const reportFile = join(output, "conversion-report.json");
  await writeJson(reportFile, {
    converter: { name: "nexo-to-craftengine", version: "0.1.0", language: "TypeScript" },
    lockedReferences: {
      nexo: { version: "1.26", jarSha256: "FA6877A46A8C2779B0B0C78C258931DC85AECDE6E70234D91EA8624F91B75B16" },
      craftEngine: { version: "26.8", commit: "c9a2ab61db6f5cea7314f506b098dea08c7bd323" },
      itemDefinitions: "Minecraft 1.21.11 client item-definition and tint semantics",
      materialSolidity: {
        minecraft: "1.21.11", paperBuild: 116, solidBlockCount: MINECRAFT_1_21_11_SOLID_BLOCK_COUNT,
        paperJarSha256: "E708E8C132DC143FFD73528CCCB9532E2EB17628B1A0EEE74469BF466C7003F8",
      },
    },
    input: root,
    output,
    options: { ...options, sourceNamespace: undefined, namespace, namespaceMode },
    identity: {
      sourcePlatform: "Nexo 1.26",
      sourceRuntimeNamespace: NEXO_ITEM_NAMESPACE,
      authorNamespace: inferredNamespace?.namespace ?? null,
      targetItemNamespace: namespace,
      namespaceMode,
      evidence: inferredNamespace?.evidence ?? "No unambiguous author namespace was found; used the Nexo runtime namespace fallback",
      candidates: inferredNamespace?.candidates ?? [],
    },
    counts: {
      sourceItems: sourceItems.length,
      templates: templateCount,
      items: Object.keys(items).length,
      furniture: Object.keys(furniture).length,
      blocks: Object.keys(blocks).length,
      recipes: Object.keys(recipes).length,
      sounds: Object.keys(sounds).length,
      glyphs: Object.keys(glyphMappings).length,
      images: Object.keys(glyphConversion.images).length,
      resources: resourceCount,
      diagnostics: diagnostics.counts(),
    },
    audit,
    success,
    diagnostics: diagnostics.items,
  });
  return {
    success, diagnostics, reportFile,
    itemCount: Object.keys(items).length,
    templateCount,
    furnitureCount: Object.keys(furniture).length,
    blockCount: Object.keys(blocks).length,
    recipeCount: Object.keys(recipes).length,
    soundCount: Object.keys(sounds).length,
    glyphCount: Object.keys(glyphConversion.images).length,
    resourceCount,
    audit,
    namespace,
    namespaceMode,
  };
}
