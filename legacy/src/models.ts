import type { DiagnosticBag } from "./diagnostics.js";
import { minecraftKey, normalizeLocation, normalizeModelLocation, normalizeTextureLocation } from "./resource-location.js";
import {
  asStringList,
  deepClone,
  findKey,
  getNumber,
  getObject,
  getString,
  getValue,
  isObject,
  type JsonObject,
  type JsonValue,
} from "./types.js";

export interface ModelReference {
  path: string;
  generation?: JsonObject;
  blueprint?: string;
  origin: "model" | "texture" | "default";
}

export interface PackModelInfo {
  hasPack: boolean;
  base: ModelReference;
  parent: string;
  customModelData?: number;
  pulling: ModelReference[];
  damaged: ModelReference[];
  composite: ModelReference[];
  dyeable?: ModelReference;
  throwing?: ModelReference;
  cast?: ModelReference;
  broken?: ModelReference;
  blocking?: ModelReference;
  charged?: ModelReference;
  firework?: ModelReference;
  handAnimationOnSwap: boolean;
  oversizedInGui: boolean;
  swapAnimationScale: number;
}

export interface ItemModelMetadata {
  handAnimationOnSwap: boolean;
  oversizedInGui: boolean;
  swapAnimationScale: number;
}

export interface ConvertedModels {
  model?: JsonValue;
  legacyModel?: JsonObject;
  baseModel?: string;
  generatedItemModel: boolean;
  metadata?: ItemModelMetadata;
  modelSemantics: JsonObject;
}

interface ModelContext {
  source: string;
  item: string;
  diagnostics: DiagnosticBag;
  modelAliases?: ReadonlyMap<string, string>;
}

function details(context: ModelContext, field: string): { source: string; item: string; field: string } {
  return { source: context.source, item: context.item, field };
}

function normalizeParent(value: string, context: ModelContext): string {
  return normalizeModelLocation(value, context.diagnostics, details(context, "Pack.parent_model")) ?? "minecraft:item/generated";
}

function normalizeStaticModel(value: string, context: ModelContext, field: string): string | undefined {
  const location = normalizeModelLocation(value, context.diagnostics, details(context, field));
  return location ? context.modelAliases?.get(location) ?? location : undefined;
}

function normalizeTextureValue(value: string, context: ModelContext, field: string): string | undefined {
  return normalizeTextureLocation(value, context.diagnostics, details(context, field));
}

function modelTextureMap(parentLocation: string, layers: string[], variables: JsonObject): JsonObject {
  if (Object.keys(variables).length > 0) return variables;
  if (layers.length === 0) return {};
  const result: JsonObject = { particle: layers[0] ?? "minecraft:missingno" };
  const parent = parentLocation.slice(parentLocation.indexOf(":") + 1);
  const layer = (index: number): string => layers[index] ?? layers[0] ?? "minecraft:missingno";
  if (parent === "block/cube" || parent === "block/cube_directional" || parent === "block/cube_mirrored") {
    Object.assign(result, { particle: layer(2), down: layer(0), up: layer(1), north: layer(2), south: layer(3), west: layer(4), east: layer(5) });
  } else if (parent === "block/cube_all" || parent === "block/cube_mirrored_all") {
    result.all = layer(0);
  } else if (parent === "block/cross") {
    result.cross = layer(0);
  } else if (parent.startsWith("block/orientable")) {
    result.front = layer(0);
    result.side = layer(1);
    if (!parent.endsWith("vertical")) result.top = layer(2);
    if (parent.endsWith("with_bottom")) result.bottom = layer(3);
  } else if (parent.startsWith("block/cube_column")) {
    result.end = layer(0);
    result.side = layer(1);
  } else if (parent === "block/cube_bottom_top" || parent.includes("block/slab") || parent.endsWith("stairs")) {
    result.bottom = layer(0);
    result.side = layer(1);
    result.top = layer(2);
  } else if (parent === "block/cube_top") {
    result.top = layer(0);
    result.side = layer(1);
  } else if (parent.includes("block/door_")) {
    result.bottom = layer(0);
    result.top = layer(1);
  } else if (parent.includes("trapdoor") || parent.includes("chain")) {
    result.texture = layer(0);
  } else if (parent.includes("lantern")) {
    result.lantern = layer(0);
  } else if (parent.includes("template_bars")) {
    result.bars = layer(0);
    result.edge = layer(1);
  } else {
    layers.forEach((value, index) => { result["layer" + index] = value; });
  }
  return result;
}

function readBaseTextures(pack: JsonObject, parent: string, context: ModelContext): JsonObject | undefined {
  const rawVariables = getObject(pack, "textures");
  const variables: JsonObject = {};
  if (rawVariables) {
    for (const [name, raw] of Object.entries(rawVariables)) {
      if (typeof raw !== "string") {
        context.diagnostics.error("PACK_TEXTURE_NOT_STRING", "Texture variable " + name + " must be a string", details(context, "Pack.textures." + name));
        continue;
      }
      const value = normalizeTextureValue(raw, context, "Pack.textures." + name);
      if (value) variables[name] = value;
    }
    return modelTextureMap(parent, [], variables);
  }
  const rawTexture = getValue(pack, "texture");
  const list = asStringList(rawTexture);
  if (list.length === 0 && Array.isArray(getValue(pack, "textures"))) list.push(...asStringList(getValue(pack, "textures")));
  const layers = list.map((value) => normalizeTextureValue(value, context, "Pack.texture")).filter((value): value is string => value !== undefined);
  return layers.length > 0 ? modelTextureMap(parent, layers, {}) : undefined;
}

function generation(parent: string, textures: JsonObject | undefined): JsonObject | undefined {
  if (!textures || Object.keys(textures).length === 0) return undefined;
  return { parent, textures };
}

function readSingleVariant(
  pack: JsonObject,
  modelKey: string,
  textureKey: string,
  base: ModelReference,
  parent: string,
  context: ModelContext,
): ModelReference | undefined {
  const model = getString(pack, modelKey);
  if (model) {
    const path = normalizeStaticModel(model, context, "Pack." + modelKey);
    return path ? { path, origin: "model" } : undefined;
  }
  const texture = getString(pack, textureKey);
  if (!texture) return undefined;
  const path = normalizeTextureValue(texture, context, "Pack." + textureKey);
  if (!path) return undefined;
  const specialTextures = modelTextureMap(base.origin === "default" ? parent : base.path, [path], {});
  return { path, origin: "texture", generation: generation(base.origin === "default" ? parent : base.path, specialTextures) };
}

function readListVariant(
  pack: JsonObject,
  modelKey: string,
  textureKey: string,
  base: ModelReference,
  parent: string,
  context: ModelContext,
): ModelReference[] {
  const models = asStringList(getValue(pack, modelKey));
  if (models.length > 0) {
    return models.map((value) => normalizeStaticModel(value, context, "Pack." + modelKey))
      .filter((value): value is string => value !== undefined)
      .map((path) => ({ path, origin: "model" as const }));
  }
  const textures = asStringList(getValue(pack, textureKey));
  return textures.map((value) => normalizeTextureValue(value, context, "Pack." + textureKey))
    .filter((value): value is string => value !== undefined)
    .map((path) => ({
      path,
      origin: "texture" as const,
      generation: generation(base.origin === "default" ? parent : base.path, modelTextureMap(base.origin === "default" ? parent : base.path, [path], {})),
    }));
}

export function readPackModel(pack: JsonObject | undefined, itemId: string, context: ModelContext): PackModelInfo {
  if (!pack) {
    return {
      hasPack: false,
      base: { path: "minecraft:" + itemId, origin: "default" },
      parent: "minecraft:item/generated",
      pulling: [], damaged: [], composite: [],
      handAnimationOnSwap: true, oversizedInGui: false, swapAnimationScale: 1,
    };
  }
  const parentRaw = getString(pack, "parent_model") ?? getString(pack, "parent") ?? "minecraft:item/generated";
  const parent = normalizeParent(parentRaw, context);
  const bbmodel = getString(pack, "bbmodel");
  const explicitModel = getString(pack, "model");
  const textures = readBaseTextures(pack, parent, context);
  let base: ModelReference;
  if (bbmodel) {
    const path = normalizeLocation(bbmodel, context.diagnostics, details(context, "Pack.bbmodel"), [".bbmodel"] ) ?? "minecraft:" + itemId;
    const separator = path.indexOf(":");
    base = { path, origin: "model", blueprint: path.slice(0, separator) + "/" + path.slice(separator + 1) };
    context.diagnostics.warning("BBMODEL_CONVERTER_REVIEW", "The .bbmodel is delegated to CraftEngine's Blockbench converter; verify rotations, animation metadata, and extracted texture paths", { ...details(context, "Pack.bbmodel"), lossy: true });
  } else if (explicitModel) {
    const path = normalizeStaticModel(explicitModel, context, "Pack.model") ?? "minecraft:" + itemId;
    base = { path, origin: "model" };
  } else {
    const path = normalizeModelLocation(itemId, context.diagnostics, details(context, "Pack.model(default)")) ?? "minecraft:" + itemId;
    base = { path, origin: textures ? "texture" : "default", generation: generation(parent, textures) };
  }
  // Nexo 1.26 does not read Pack.generate_model when choosing between an
  // explicit model and generated textures. Matching that parser behavior is
  // exact and does not require a per-item diagnostic.
  const customModelData = getNumber(pack, "custom_model_data");
  if (customModelData !== undefined && (!Number.isInteger(customModelData) || customModelData <= 0 || customModelData > 16_777_216)) {
    context.diagnostics.error("INVALID_CUSTOM_MODEL_DATA", "custom_model_data must be an integer in 1..16777216", details(context, "Pack.custom_model_data"));
  }
  const info: PackModelInfo = {
    hasPack: true,
    base,
    parent,
    customModelData: customModelData !== undefined && Number.isInteger(customModelData) && customModelData > 0 ? customModelData : undefined,
    pulling: [], damaged: [], composite: [],
    handAnimationOnSwap: typeof getValue(pack, "hand_swap_animation") === "boolean" ? getValue(pack, "hand_swap_animation") as boolean : true,
    oversizedInGui: typeof getValue(pack, "oversized_in_gui") === "boolean" ? getValue(pack, "oversized_in_gui") as boolean : false,
    swapAnimationScale: getNumber(pack, "swap_animation_scale") ?? 1,
  };
  info.blocking = readSingleVariant(pack, "blocking_model", "blocking_texture", base, parent, context);
  info.charged = readSingleVariant(pack, "charged_model", "charged_texture", base, parent, context);
  info.cast = readSingleVariant(pack, "cast_model", "cast_texture", base, parent, context);
  info.broken = readSingleVariant(pack, "broken_model", "broken_texture", base, parent, context);
  info.firework = readSingleVariant(pack, "firework_model", "firework_texture", base, parent, context);
  info.dyeable = readSingleVariant(pack, "dyeable_model", "dyeable_texture", base, parent, context);
  info.throwing = readSingleVariant(pack, "throwing_model", "throwing_texture", base, parent, context);
  info.pulling = readListVariant(pack, "pulling_models", "pulling_textures", base, parent, context);
  info.damaged = readListVariant(pack, "damaged_models", "damaged_textures", base, parent, context);
  info.composite = readListVariant(pack, "composite_models", "composite_textures", base, parent, context);
  return info;
}

function modelNode(reference: ModelReference, tints?: JsonValue[]): JsonObject {
  const node: JsonObject = { type: "model", path: reference.path };
  if (tints && tints.length > 0) node.tints = tints;
  if (reference.generation) node.generation = deepClone(reference.generation);
  if (reference.blueprint) node.blueprint = reference.blueprint;
  return node;
}

function roundPredicate(value: number, maximum: number): number {
  return Math.min(Number((Math.round(value / 0.05) * 0.05).toFixed(2)), maximum);
}

function colorInteger(raw: JsonValue | undefined): number {
  if (typeof raw === "number" && Number.isInteger(raw)) return raw & 0xffffff;
  if (typeof raw !== "string") return 0xffffff;
  const text = raw.trim();
  if (/^#[0-9a-f]{6}$/i.test(text)) return Number.parseInt(text.slice(1), 16);
  const parts = text.split(",").map((entry) => Number(entry.trim()));
  if (parts.length === 3 && parts.every((entry) => Number.isInteger(entry) && entry >= 0 && entry <= 255)) {
    return ((parts[0] ?? 255) << 16) | ((parts[1] ?? 255) << 8) | (parts[2] ?? 255);
  }
  return 0xffffff;
}

const VANILLA_REFERENCE_TINTS_1_21_11: Record<string, JsonValue[]> = {
  fern: [{ type: "grass", downfall: 1, temperature: 0.5 }],
  filled_map: [{ type: "constant", value: -1 }, { type: "map_color", default: 4603950 }],
  firework_star: [{ type: "constant", value: -1 }, { type: "firework", default: -7697782 }],
  grass_block: [{ type: "grass", downfall: 1, temperature: 0.5 }],
  large_fern: [{ type: "grass", downfall: 1, temperature: 0.5 }],
  leather_horse_armor: [{ type: "dye", default: -6265536 }],
  lingering_potion: [{ type: "potion", default: -13083194 }],
  potion: [{ type: "potion", default: -13083194 }],
  short_grass: [{ type: "grass", downfall: 1, temperature: 0.5 }],
  splash_potion: [{ type: "potion", default: -13083194 }],
  tall_grass: [{ type: "grass", downfall: 1, temperature: 0.5 }],
  tipped_arrow: [{ type: "potion", default: -13083194 }],
};

function buildShortcutModern(info: PackModelInfo, material: string, color: JsonValue | undefined): JsonObject {
  // Nexo inherits tints only when the vanilla item's top-level 1.21.11 model is a simple reference.
  const tintSources: JsonValue[] = color === undefined
    ? deepClone(VANILLA_REFERENCE_TINTS_1_21_11[material] ?? [])
    : [{ type: "dye", default: colorInteger(color) }];
  const referenceTints: JsonValue[] = tintSources.length > 0 ? tintSources : [{ type: "dye", default: -1 }];
  const base = modelNode(info.base, tintSources);
  const referenceBase = modelNode(info.base, referenceTints);
  let primary: JsonObject;
  if (info.pulling.length > 0) {
    const entries: JsonValue[] = info.pulling.map((reference, index) => ({
      threshold: index === 0 ? 0 : roundPredicate((index + 1) / info.pulling.length, 0.9),
      model: modelNode(reference, referenceTints),
    }));
    const pulling: JsonObject = {
      type: "condition",
      property: "using_item",
      on_true: {
        type: "range_dispatch",
        property: material === "crossbow" ? "crossbow/pull" : "use_duration",
        scale: material === "crossbow" ? 1 : 0.05,
        entries,
      },
      on_false: referenceBase,
    };
    if (material === "crossbow" && (info.charged || info.firework)) {
      const cases: JsonValue[] = [];
      if (info.charged) cases.push({ when: "arrow", model: modelNode(info.charged, referenceTints) });
      if (info.firework) cases.push({ when: "rocket", model: modelNode(info.firework, referenceTints) });
      primary = { type: "select", property: "charge_type", cases, fallback: pulling };
    } else primary = pulling;
  } else if (info.dyeable) {
    primary = {
      type: "condition",
      property: "has_component",
      component: "minecraft:dyed_color",
      on_true: modelNode(info.dyeable, [{ type: "dye", default: colorInteger(color) }]),
      on_false: modelNode(info.base),
    };
  } else {
    const selected = info.cast ?? info.broken ?? info.throwing ?? info.blocking;
    if (selected) {
      let property = "using_item";
      if (info.cast) property = "fishing_rod/cast";
      else if (info.broken) property = "broken";
      primary = { type: "condition", property, on_true: modelNode(selected, referenceTints), on_false: referenceBase };
    } else if (material === "player_head") {
      primary = { type: "special", base: info.base.path, model: { type: "player_head" } };
      if (info.base.generation) primary.generation = deepClone(info.base.generation);
      if (info.base.blueprint) primary.blueprint = info.base.blueprint;
    } else primary = base;
  }
  if (info.composite.length > 0) {
    primary = { type: "composite", models: [primary, ...info.composite.map((entry) => modelNode(entry))] };
  }
  return primary;
}

function normalizeAstValue(value: JsonValue, context: ModelContext, parentKey = "", nodeType = ""): JsonValue {
  if (Array.isArray(value)) return value.map((entry) => normalizeAstValue(entry, context, parentKey, nodeType));
  if (!isObject(value)) {
    if (typeof value === "string" && parentKey === "model" && nodeType === "model") {
      return normalizeModelLocation(value, context.diagnostics, details(context, "ItemModel.model")) ?? value;
    }
    return value;
  }
  const rawType = typeof value.type === "string" ? minecraftKey(value.type) : nodeType;
  const output: JsonObject = {};
  for (const [rawKey, rawValue] of Object.entries(value)) {
    const key = rawKey.replaceAll("-", "_");
    if (key === "type" && typeof rawValue === "string") output.type = minecraftKey(rawValue);
    else if (key === "property" && typeof rawValue === "string") output.property = minecraftKey(rawValue);
    else if (key === "component" && typeof rawValue === "string") {
      output.component = normalizeLocation(rawValue, context.diagnostics, details(context, "ItemModel.component")) ?? rawValue;
    } else if ((key === "model" || key === "path") && typeof rawValue === "string" && rawType === "model") {
      output.path = normalizeModelLocation(rawValue, context.diagnostics, details(context, "ItemModel.model")) ?? rawValue;
    } else {
      output[key] = normalizeAstValue(rawValue, context, key, rawType);
    }
  }
  return output;
}

function explicitItemModelMetadata(raw: JsonObject): ItemModelMetadata {
  return {
    handAnimationOnSwap: typeof getValue(raw, "hand_animation_on_swap") === "boolean" ? getValue(raw, "hand_animation_on_swap") as boolean : true,
    oversizedInGui: typeof getValue(raw, "oversized_in_gui") === "boolean" ? getValue(raw, "oversized_in_gui") as boolean : false,
    swapAnimationScale: getNumber(raw, "swap_animation_scale") ?? 1,
  };
}

export function convertExplicitItemModel(raw: JsonObject | undefined, context: ModelContext): JsonObject | undefined {
  if (!raw) return undefined;
  const metadataKeys = new Set(["hand_animation_on_swap", "oversized_in_gui", "swap_animation_scale"]);
  const body = Object.fromEntries(Object.entries(raw).filter(([key]) => !metadataKeys.has(key.toLowerCase()))) as JsonObject;
  const normalized = normalizeAstValue(body, context);
  return isObject(normalized) ? normalized : undefined;
}

function legacyReference(reference: ModelReference, predicate?: JsonObject): JsonObject {
  const result: JsonObject = { path: reference.path };
  if (predicate) result.predicate = predicate;
  if (reference.generation) result.generation = deepClone(reference.generation);
  if (reference.blueprint) result.blueprint = reference.blueprint;
  return result;
}

export function buildLegacyModel(info: PackModelInfo): JsonObject {
  const result = legacyReference(info.base);
  const overrides: JsonValue[] = [];
  if (info.blocking) overrides.push(legacyReference(info.blocking, { blocking: 1 }));
  if (info.charged) overrides.push(legacyReference(info.charged, { charged: 1 }));
  if (info.cast) overrides.push(legacyReference(info.cast, { cast: 1 }));
  if (info.broken) overrides.push(legacyReference(info.broken, { broken: 1 }));
  if (info.firework) overrides.push(legacyReference(info.firework, { firework: 1 }));
  info.pulling.forEach((reference, index) => {
    const pull = index === 0 ? 0 : roundPredicate((index + 1) / info.pulling.length, 0.9);
    overrides.push(legacyReference(reference, { pulling: 1, pull }));
  });
  info.damaged.slice(1).forEach((reference, offset) => {
    const index = offset + 1;
    overrides.push(legacyReference(reference, { pulling: 1, damage: roundPredicate(index / info.damaged.length, 0.99) }));
  });
  if (overrides.length > 0) result.overrides = overrides;
  return result;
}

export function convertModels(
  info: PackModelInfo,
  explicitItemModel: JsonObject | undefined,
  material: string,
  color: JsonValue | undefined,
  clientMode: "modern" | "hybrid" | "legacy",
  context: ModelContext,
): ConvertedModels {
  if (!info.hasPack && !explicitItemModel) return { generatedItemModel: false, modelSemantics: {} };
  const hasShortcut = info.pulling.length > 0 || info.composite.length > 0 || Boolean(info.dyeable || info.cast || info.broken || info.throwing || info.blocking || info.charged || info.firework);
  const modern = explicitItemModel && !hasShortcut
    ? convertExplicitItemModel(explicitItemModel, context)
    : buildShortcutModern(info, material, color);
  const model = clientMode === "legacy" ? modelNode(info.base) : modern;
  const metadata: ItemModelMetadata = explicitItemModel && !hasShortcut && clientMode !== "legacy"
    ? explicitItemModelMetadata(explicitItemModel)
    : { handAnimationOnSwap: info.handAnimationOnSwap, oversizedInGui: info.oversizedInGui, swapAnimationScale: info.swapAnimationScale };
  const converted: ConvertedModels = {
    model,
    baseModel: info.base.path,
    generatedItemModel: model !== undefined,
    metadata: model !== undefined ? metadata : undefined,
    modelSemantics: {
      base_model: info.base.path,
      modern_source: explicitItemModel ? "ItemModel_or_Pack_shortcut_priority" : "Pack",
      pulling_thresholds: info.pulling.map((_, index) => index === 0 ? 0 : roundPredicate((index + 1) / info.pulling.length, 0.9)),
      damaged_legacy_only: info.damaged.length > 0,
    },
  };
  if (clientMode !== "modern") converted.legacyModel = buildLegacyModel(info);
  if (info.damaged.length > 0) {
    context.diagnostics.warning(
      "NEXO_DAMAGED_MODEL_LEGACY_QUIRK",
      "Nexo 1.26 only consumes damaged_models in legacy overrides and also adds pulling=1; the converter preserves that actual behavior",
      { ...details(context, "Pack.damaged_models"), lossy: false },
    );
  }
  if (explicitItemModel && hasShortcut) {
    context.diagnostics.info("PACK_SHORTCUT_PRECEDENCE", "Nexo Pack shortcut fields take precedence over the explicit ItemModel in Nexo 1.26", details(context, "ItemModel"));
  }
  return converted;
}
