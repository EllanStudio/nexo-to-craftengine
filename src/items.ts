import { convertNexoBuilderComponent } from "./component-builders.js";
import type { DiagnosticBag } from "./diagnostics.js";
import { BUKKIT_MATERIALS_1_21_11 } from "./materials-1.21.11.js";
import { convertModels, readPackModel } from "./models.js";
import { normalizeLocation, normalizeModelLocation } from "./resource-location.js";
import {
  asStringList,
  deepClone,
  deepMerge,
  findKey,
  getNumber,
  getObject,
  getString,
  getValue,
  isObject,
  type JsonObject,
  type JsonValue,
  withoutKeys,
} from "./types.js";

export interface SourceItem {
  id: string;
  source: string;
  config: JsonObject;
  template: boolean;
}

export interface ResolvedItem extends SourceItem {
  config: JsonObject;
  templateIds: string[];
}

export interface ItemOptions {
  namespace: string;
  clientMode: "modern" | "hybrid" | "legacy";
  modelAliases?: ReadonlyMap<string, string>;
}

export interface ConvertedItem {
  sourceId: string;
  targetId: string;
  config: JsonObject;
  modelPointer?: string;
  baseModel?: string;
  semantics: JsonObject;
}

export function matchBukkitMaterial(value: JsonValue | undefined): string | undefined {
  if (typeof value !== "string") return undefined;
  const candidate = (value.startsWith("minecraft:") ? value.slice("minecraft:".length) : value)
    .toUpperCase()
    .replace(/\s+/g, "_")
    .replace(/\W/g, "")
    .toLowerCase();
  return BUKKIT_MATERIALS_1_21_11.has(candidate) ? candidate : undefined;
}

function capitalizeId(id: string): string {
  return id.split("_").map((part) => part.length > 0 ? part.charAt(0).toUpperCase() + part.slice(1) : part).join(" ");
}

function placeholderValues(id: string, item: JsonObject): Record<string, string | string[]> {
  const pack = getObject(item, "Pack");
  return {
    item_id: id,
    item_id_capitalized: capitalizeId(id),
    lore: asStringList(getValue(item, "lore")),
    parent: pack ? getString(pack, "parent_model") ?? getString(pack, "parent") ?? "minecraft:item/generated" : "minecraft:item/generated",
    model: pack ? getString(pack, "model") ?? id : id,
    texture: pack ? asStringList(getValue(pack, "texture")) : [],
  };
}

function expandString(input: string, replacements: Record<string, string | string[]>): string | string[] {
  let values = [input];
  for (const [key, replacement] of Object.entries(replacements)) {
    const token = "<" + key + ">";
    if (!values.some((value) => value.includes(token))) continue;
    const alternatives = typeof replacement === "string" ? [replacement] : replacement;
    if (alternatives.length === 0) return [];
    values = values.flatMap((value) => alternatives.map((entry) => value.replaceAll(token, entry)));
  }
  return values.length === 1 ? values[0] ?? "" : values;
}

function applyPlaceholders(value: JsonValue, replacements: Record<string, string | string[]>): JsonValue {
  if (typeof value === "string") return expandString(value, replacements);
  if (Array.isArray(value)) {
    return value.flatMap((entry) => {
      const converted = applyPlaceholders(entry, replacements);
      return Array.isArray(converted) && typeof entry === "string" ? converted : [converted];
    });
  }
  if (!isObject(value)) return value;
  const result: JsonObject = {};
  for (const [key, entry] of Object.entries(value)) result[key] = applyPlaceholders(entry, replacements);
  return result;
}

function hasPlaceholder(value: JsonValue): boolean {
  if (typeof value === "string") return /<(item_id|item_id_capitalized|lore|parent|model|texture)>/.test(value);
  if (Array.isArray(value)) return value.some(hasPlaceholder);
  return isObject(value) && Object.values(value).some(hasPlaceholder);
}

export function identifyTemplates(items: SourceItem[]): Set<string> {
  const referenced = new Set<string>();
  for (const item of items) for (const id of asStringList(getValue(item.config, "template"))) referenced.add(id);
  return referenced;
}

export function resolveItemTemplates(items: SourceItem[], diagnostics: DiagnosticBag): ResolvedItem[] {
  const byId = new Map(items.map((item) => [item.id, item]));
  const templateIds = identifyTemplates(items);
  const cache = new Map<string, JsonObject>();

  const resolveConfig = (item: SourceItem, stack: string[]): JsonObject => {
    const cached = cache.get(item.id);
    if (cached) return deepClone(cached);
    if (stack.includes(item.id)) {
      diagnostics.error("TEMPLATE_CYCLE", "Template cycle: " + [...stack, item.id].join(" -> "), { source: item.source, item: item.id, field: "template" });
      return deepClone(item.config);
    }
    let merged: JsonObject = {};
    const references = asStringList(getValue(item.config, "template"));
    for (const templateId of references) {
      const template = byId.get(templateId);
      if (!template) {
        diagnostics.error("TEMPLATE_NOT_FOUND", "Nexo template not found: " + templateId, { source: item.source, item: item.id, field: "template", lossy: true });
        continue;
      }
      const resolvedTemplate = withoutKeys(resolveConfig(template, [...stack, item.id]), ["injectId"]);
      merged = deepMerge(merged, resolvedTemplate);
    }
    const ownConfig = withoutKeys(item.config, ["template"]);
    if (Object.prototype.hasOwnProperty.call(ownConfig, "material") && !matchBukkitMaterial(ownConfig.material)) {
      diagnostics.info("INVALID_MATERIAL_INHERITED", "Nexo ignores an invalid material and inherits its template material, or PAPER when no template supplies one", { source: item.source, item: item.id, field: "material" });
      delete ownConfig.material;
    }
    merged = deepMerge(merged, ownConfig);
    cache.set(item.id, deepClone(merged));
    return merged;
  };

  return items.map((item) => {
    const raw = resolveConfig(item, []);
    const converted = applyPlaceholders(raw, placeholderValues(item.id, item.config));
    const config = isObject(converted) ? converted : raw;
    if (hasPlaceholder(config)) {
      diagnostics.error("TEMPLATE_PLACEHOLDER_UNRESOLVED", "A supported Nexo template placeholder could not be resolved", { source: item.source, item: item.id, lossy: true });
    }
    return {
      ...item,
      template: templateIds.has(item.id),
      templateIds: asStringList(getValue(item.config, "template")),
      config,
    };
  });
}

function normalizeComponentName(name: string): string | undefined {
  const separator = name.indexOf(":");
  const namespace = separator < 0 ? "minecraft" : name.slice(0, separator);
  const path = separator < 0 ? name : name.slice(separator + 1);
  if (!/^[a-z0-9_.-]+$/.test(namespace) || !/^[a-z0-9/._-]+$/.test(path)) return undefined;
  return namespace === "minecraft" ? path : namespace + ":" + path;
}

const NEXO_COMPONENT_KEYS = new Set([
  "unset_components", "can_place_on", "can_break", "custom_data", "max_stack_size", "instrument", "enchantment_glint_override",
  "max_damage", "rarity", "food", "tool", "painting_variant", "tooltip_style", "item_model", "jukebox_playable", "use_remainder",
  "death_protection", "use_cooldown", "damage_resistant", "consumable", "equippable", "enchantable", "glider", "repairable", "profile",
  "custom_model_data", "tooltip_display", "break_sound", "weapon", "blocks_attacks", "attack_range", "kinetic_weapon", "piercing_weapon",
  "minimum_attack_charge", "swing_animation", "use_effects", "damage_type",
]);
function bukkitInt(value: JsonValue | undefined): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.trunc(value) : 0;
}

function bukkitFloat(value: JsonValue | undefined, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function bukkitStringList(value: JsonValue | undefined): string[] {
  if (typeof value === "string") return value.length > 0 ? [value] : [];
  if (!Array.isArray(value)) return [];
  return value
    .filter((entry): entry is string | number | boolean => typeof entry === "string" || typeof entry === "number" || typeof entry === "boolean")
    .map(String)
    .filter((entry) => entry.length > 0);
}

function componentLocation(value: JsonValue | undefined, key: string, diagnostics: DiagnosticBag, source: string, item: string): string | undefined {
  if (typeof value !== "string" || value.length === 0) return undefined;
  return normalizeLocation(value, diagnostics, { source, item, field: "Components." + key });
}

function durationSeconds(value: JsonValue | undefined): number {
  if (typeof value === "number" && Number.isFinite(value)) return Math.max(0, value);
  if (typeof value !== "string") return 0;
  const match = /^\s*(-?(?:\d+(?:\.\d*)?|\.\d+))\s*(ms|ticks?|t|s|sec(?:onds?)?|m|min(?:utes?)?|h|hours?)?\s*$/i.exec(value);
  if (!match) return 0;
  const amount = Number(match[1]);
  const unit = (match[2] ?? "s").toLowerCase();
  const multiplier = unit === "ms" ? 0.001 : unit === "t" || unit.startsWith("tick") ? 0.05 : unit === "m" || unit.startsWith("min") ? 60 : unit === "h" || unit.startsWith("hour") ? 3600 : 1;
  return Math.max(0, amount * multiplier);
}

function sectionEntries(value: JsonValue | undefined): JsonObject[] {
  if (Array.isArray(value)) return value.filter(isObject);
  if (!isObject(value)) return [];
  if (typeof value.name === "string" || typeof value.value === "string") return [value];
  return Object.values(value).filter(isObject);
}

function convertProfile(raw: JsonObject, diagnostics: DiagnosticBag, source: string, item: string): JsonObject | undefined {
  const result: JsonObject = {};
  if (typeof raw.name === "string" && raw.name.length > 0) result.name = raw.name;
  if (typeof raw.uuid === "string") {
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(raw.uuid)) {
      diagnostics.error("COMPONENT_PROFILE_UUID_INVALID", "Nexo UUID.fromString rejects this Components.profile.uuid", { source, item, field: "Components.profile.uuid" });
      return undefined;
    }
    result.id = raw.uuid;
  }
  const properties = [...sectionEntries(raw.property), ...sectionEntries(raw.properties)];
  const converted: JsonObject[] = [];
  for (const [index, property] of properties.entries()) {
    if (typeof property.name !== "string" || typeof property.value !== "string") {
      diagnostics.error("COMPONENT_PROFILE_PROPERTY_INVALID", "Nexo requires string name and value for every profile property", { source, item, field: "Components.profile.properties[" + index + "]" });
      return undefined;
    }
    const entry: JsonObject = { name: property.name, value: property.value };
    if (typeof property.signature === "string") entry.signature = property.signature;
    converted.push(entry);
  }
  if (converted.length > 0) result.properties = converted;
  return result;
}

function convertCustomModelData(raw: JsonObject, diagnostics: DiagnosticBag, source: string, item: string): JsonObject | undefined {
  const colors = bukkitStringList(raw.color ?? raw.colors).map(nexoColor).filter((value): value is number => value !== undefined);
  const floats: number[] = [];
  for (const [index, text] of bukkitStringList(raw.float ?? raw.floats).entries()) {
    const parsed = Number(text);
    if (!Number.isFinite(parsed)) {
      diagnostics.error("COMPONENT_CMD_FLOAT_INVALID", "Nexo Float.parseFloat rejects this custom_model_data float", { source, item, field: "Components.custom_model_data.float[" + index + "]" });
      return undefined;
    }
    floats.push(parsed);
  }
  const strings = bukkitStringList(raw.string ?? raw.strings);
  const flags = bukkitStringList(raw.flag ?? raw.flags).filter((text) => text === "true" || text === "false").map((text) => text === "true");
  const result: JsonObject = {};
  if (colors.length > 0) result.colors = colors;
  if (floats.length > 0) result.floats = floats;
  if (strings.length > 0) result.strings = strings;
  if (flags.length > 0) result.flags = flags;
  return result;
}

function mapComponents(components: JsonObject, data: JsonObject, diagnostics: DiagnosticBag, source: string, item: string, material: string): string | undefined {
  const copied: JsonObject = {};
  let itemModel: string | undefined;
  for (const [key, rawValue] of Object.entries(components)) {
    // Bukkit ConfigurationSection paths and Nexo's parser are case-sensitive;
    // do not invent aliases by lowercasing source keys.
    if (key === "unset_components" || key === "unset_component") continue;
    if (key === "potion_contents") {
      diagnostics.info("NEXO_COMPONENT_POTION_CONTENTS_IGNORED", "Nexo 1.26 ComponentParser does not recognize Components.potion_contents; it was intentionally not emitted", { source, item, field: "Components.potion_contents" });
      continue;
    }
    if (!NEXO_COMPONENT_KEYS.has(key)) {
      diagnostics.info("NEXO_COMPONENT_UNKNOWN_IGNORED", "Nexo 1.26 ComponentParser ignores unsupported or differently-cased component key " + key, { source, item, field: "Components." + key });
      continue;
    }
    if (key === "item_model") {
      if (typeof rawValue !== "string") diagnostics.error("ITEM_MODEL_COMPONENT_INVALID", "Components.item_model must be a resource-location string", { source, item, field: "Components.item_model" });
      else itemModel = normalizeLocation(rawValue, diagnostics, { source, item, field: "Components.item_model" });
      continue;
    }
    const builder = convertNexoBuilderComponent(key, rawValue, diagnostics, source, item, components, material);
    if (builder) {
      if (builder.status === "manual") {
        diagnostics.warning("COMPONENT_CODEC_MANUAL", "Components." + key + " cannot be resolved statically: " + (builder.reason ?? "runtime registry data is required"), { source, item, field: "Components." + key, lossy: true });
      } else if (builder.value !== undefined) {
        copied[key] = builder.value;
      }
      continue;
    }
    if (key === "custom_data") {
      if (isObject(rawValue)) copied.custom_data = deepClone(rawValue);
    } else if (key === "max_stack_size") {
      copied.max_stack_size = Math.max(1, Math.min(99, bukkitInt(rawValue)));
    } else if (key === "enchantment_glint_override") {
      if (typeof rawValue === "boolean") copied.enchantment_glint_override = rawValue;
      else if (rawValue === "true" || rawValue === "false") copied.enchantment_glint_override = rawValue === "true";
    } else if (key === "max_damage") {
      copied.max_damage = Math.max(1, bukkitInt(rawValue));
    } else if (key === "rarity") {
      if (typeof rawValue === "string" && ["common", "uncommon", "rare", "epic"].includes(rawValue.toLowerCase())) copied.rarity = rawValue.toLowerCase();
    } else if (key === "food") {
      if (isObject(rawValue)) copied.food = {
        nutrition: bukkitInt(rawValue.nutrition), saturation: bukkitFloat(rawValue.saturation), can_always_eat: rawValue.can_always_eat === true,
      };
    } else if (key === "painting_variant") {
      const value = componentLocation(rawValue, key, diagnostics, source, item);
      if (value) copied["painting/variant"] = value;
    } else if (["instrument", "tooltip_style", "break_sound", "damage_type"].includes(key)) {
      const value = componentLocation(rawValue, key, diagnostics, source, item);
      if (value) {
        copied[key] = value;
        diagnostics.info("COMPONENT_REGISTRY_UNVERIFIED", "Registry-backed component " + key + " was syntax-validated but must exist on the target server", { source, item, field: "Components." + key });
      }
    } else if (key === "use_cooldown") {
      if (isObject(rawValue)) {
        const group = componentLocation(rawValue.group, "use_cooldown.group", diagnostics, source, item) ?? "nexo:" + item;
        copied.use_cooldown = { seconds: durationSeconds(rawValue.duration), cooldown_group: group };
      }
    } else if (key === "damage_resistant") {
      const value = componentLocation(rawValue, key, diagnostics, source, item);
      if (value) {
        copied.damage_resistant = { types: "#" + value };
        diagnostics.info("COMPONENT_REGISTRY_UNVERIFIED", "Damage-type tag existence must be checked on the target server", { source, item, field: "Components.damage_resistant" });
      }
    } else if (key === "enchantable") {
      copied.enchantable = Math.max(1, bukkitInt(rawValue));
    } else if (key === "glider") {
      if (rawValue === true) copied.glider = {};
    } else if (key === "profile") {
      if (isObject(rawValue)) {
        const profile = convertProfile(rawValue, diagnostics, source, item);
        if (profile) copied.profile = profile;
      }
    } else if (key === "custom_model_data") {
      if (isObject(rawValue)) {
        const customModelData = convertCustomModelData(rawValue, diagnostics, source, item);
        if (customModelData) copied.custom_model_data = customModelData;
      }
    } else if (key === "tooltip_display") {
      const hidden = asStringList(rawValue)
        .map((value) => normalizeLocation(value, diagnostics, { source, item, field: "Components.tooltip_display" }))
        .filter((value): value is string => value !== undefined);
      if (hidden.length > 0) copied.tooltip_display = { hide_tooltip: false, hidden_components: hidden };
    } else if (key === "minimum_attack_charge") {
      if (typeof rawValue === "number" && Number.isFinite(rawValue)) copied.minimum_attack_charge = Math.max(0, Math.min(1, rawValue));
    }
  }
  if (Object.keys(copied).length > 0) data.components = copied;
  return itemModel;
}

const VANILLA_EFFECT_IDS_1_21_11 = [
  "speed", "slowness", "haste", "mining_fatigue", "strength", "instant_health", "instant_damage", "jump_boost", "nausea",
  "regeneration", "resistance", "fire_resistance", "water_breathing", "invisibility", "blindness", "night_vision", "hunger",
  "weakness", "poison", "wither", "health_boost", "absorption", "saturation", "glowing", "levitation", "luck", "unluck",
  "slow_falling", "conduit_power", "dolphins_grace", "bad_omen", "hero_of_the_village", "darkness", "trial_omen", "raid_omen",
  "wind_charged", "weaving", "oozing", "infested", "breath_of_the_nautilus",
] as const;
const VANILLA_EFFECTS_1_21_11 = new Set<string>(VANILLA_EFFECT_IDS_1_21_11);

function resolvePotionType(raw: string): string | undefined {
  const normalized = raw.toLowerCase();
  const separator = normalized.indexOf(":");
  const namespace = separator < 0 ? "minecraft" : normalized.slice(0, separator);
  const path = separator < 0 ? normalized : normalized.slice(separator + 1);
  if (namespace !== "minecraft") return undefined;
  return VANILLA_EFFECTS_1_21_11.has(path) ? "minecraft:" + path : undefined;
}

function resolveDirectPotionEffect(raw: JsonValue | undefined, diagnostics: DiagnosticBag, source: string, item: string, index: number): string | undefined {
  if (typeof raw === "number" && Number.isInteger(raw)) {
    const path = VANILLA_EFFECT_IDS_1_21_11[raw - 1];
    return path ? "minecraft:" + path : undefined;
  }
  if (typeof raw !== "string") return undefined;
  const normalized = normalizeLocation(raw, diagnostics, { source, item, field: "PotionEffects[" + index + "].effect" });
  if (!normalized) return undefined;
  const [namespace, path] = normalized.split(":", 2) as [string, string];
  if (namespace === "minecraft" && !VANILLA_EFFECTS_1_21_11.has(path)) return undefined;
  return normalized;
}

function convertPotionEffects(value: JsonValue | undefined, diagnostics: DiagnosticBag, source: string, item: string): JsonObject[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) {
    diagnostics.info("POTION_EFFECTS_NON_LIST_IGNORED", "Nexo PotionEffects only accepts a YAML list; this value is ignored by Nexo 1.26", { source, item, field: "PotionEffects" });
    return [];
  }
  const output: JsonObject[] = [];
  for (const [index, raw] of value.entries()) {
    if (!isObject(raw)) continue; // linkedMapList filters non-map entries.
    let effectiveRaw = raw.effect;
    if (typeof raw.type === "string") {
      const fromType = resolvePotionType(raw.type);
      if (fromType) effectiveRaw = fromType;
      else if (raw.type.includes(":")) {
        diagnostics.error("POTION_EFFECT_CUSTOM_TYPE_UNREPRESENTABLE", "Nexo resolves a namespaced type and then discards its namespace before Bukkit deserialization; this custom effect cannot be migrated reliably", { source, item, field: "PotionEffects[" + index + "].type", lossy: true });
        continue;
      }
    }
    const id = resolveDirectPotionEffect(effectiveRaw, diagnostics, source, item, index);
    const duration = raw.duration;
    const amplifier = raw.amplifier;
    if (!id) {
      diagnostics.error("POTION_EFFECT_TYPE_INVALID", "PotionEffects entry has no Bukkit-resolvable effect type", { source, item, field: "PotionEffects[" + index + "].type", lossy: true });
      continue;
    }
    const validInt = (candidate: JsonValue | undefined): candidate is number => typeof candidate === "number" && Number.isInteger(candidate) && candidate >= -2147483648 && candidate <= 2147483647;
    if (!validInt(duration) || !validInt(amplifier)) {
      diagnostics.error("POTION_EFFECT_INTEGER_REQUIRED", "Bukkit PotionEffect requires integer duration and amplifier fields", { source, item, field: "PotionEffects[" + index + "]", lossy: true });
      continue;
    }
    if (raw.hidden_effect !== undefined || raw["hidden-potion-effect"] !== undefined) {
      diagnostics.error("POTION_HIDDEN_EFFECT_UNREPRESENTABLE", "Nexo's raw linked-map path cannot deserialize a nested hidden PotionEffect from this YAML form", { source, item, field: "PotionEffects[" + index + "]", lossy: true });
      continue;
    }
    const ambient = typeof raw.ambient === "boolean" ? raw.ambient : false;
    const particles = typeof raw["has-particles"] === "boolean" ? raw["has-particles"] : true;
    const icon = typeof raw["has-icon"] === "boolean" ? raw["has-icon"] : particles;
    output.push({ id, duration, amplifier, ambient, show_particles: particles, show_icon: icon });
  }
  return output;
}

const NAMED_COLORS: Record<string, number> = {
  black: 0x000000, dark_blue: 0x0000aa, dark_green: 0x00aa00, dark_aqua: 0x00aaaa,
  dark_red: 0xaa0000, dark_purple: 0xaa00aa, gold: 0xffaa00, gray: 0xaaaaaa,
  dark_gray: 0x555555, blue: 0x5555ff, green: 0x55ff55, aqua: 0x55ffff,
  red: 0xff5555, light_purple: 0xff55ff, yellow: 0xffff55, white: 0xffffff,
};

function nexoColor(raw: JsonValue | undefined): number | undefined {
  if (typeof raw !== "number" && typeof raw !== "string") return undefined;
  const text = String(raw);
  try {
    if (text.startsWith("#") || text.startsWith("0x")) {
      const hex = text.replace(/^#|^0x/, "").padStart(8, "F").slice(0, 8);
      if (!/^[0-9a-f]{8}$/i.test(hex)) return undefined;
      return Number.parseInt(hex, 16) & 0xffffff;
    }
    if (text.includes(",")) {
      const parts = text.replaceAll(" ", "").split(",");
      if ((parts.length !== 3 && parts.length !== 4) || !parts.every((part) => /^-?\d+$/.test(part))) return undefined;
      const values = parts.map(Number);
      const rgb = parts.length === 3 ? values : values.slice(1);
      if (!rgb.every((part) => part >= 0 && part <= 255)) return undefined;
      return ((rgb[0] ?? 0) << 16) | ((rgb[1] ?? 0) << 8) | (rgb[2] ?? 0);
    }
    if (/^-?\d+$/.test(text)) {
      const value = Number(text);
      return value >= 0 && value <= 0xffffff ? value : undefined;
    }
    return NAMED_COLORS[text];
  } catch {
    return undefined;
  }
}

function sectionList(value: JsonValue | undefined): JsonObject[] {
  if (Array.isArray(value)) return value.filter(isObject);
  if (!isObject(value)) return [];
  if (Object.values(value).every(isObject) && !findKey(value, "attribute") && !findKey(value, "key")) return Object.values(value).filter(isObject);
  return [value];
}

function convertAttributes(value: JsonValue | undefined, diagnostics: DiagnosticBag, source: string, item: string): JsonValue[] | undefined {
  const converted: JsonValue[] = [];
  for (const [index, modifier] of sectionList(value).entries()) {
    const rawAttribute = getString(modifier, "attribute");
    const amount = getNumber(modifier, "amount");
    if (!rawAttribute || amount === undefined) {
      diagnostics.warning("ATTRIBUTE_MODIFIER_INVALID", "Nexo ignores an attribute modifier without a valid attribute and amount", { source, item, field: "AttributeModifiers[" + index + "]", lossy: true });
      continue;
    }
    const stripped = rawAttribute.toLowerCase().replace(/^generic_/, "").replace(/^player_/, "");
    const type = stripped.includes(":") ? stripped : "minecraft:" + stripped;
    const operationMap: Record<string, string> = {
      add_number: "add_value",
      add_scalar: "add_multiplied_base",
      multiply_scalar_1: "add_multiplied_total",
    };
    const operation = operationMap[(getString(modifier, "operation") ?? "ADD_NUMBER").toLowerCase()] ?? "add_value";
    const path = stripped.slice(stripped.indexOf(":") + 1).replace(/^[^.]+\./, "");
    const output: JsonObject = {
      type,
      slot: (getString(modifier, "slot") ?? "any").toLowerCase(),
      id: (getString(modifier, "key") ?? "nexo:" + item + "_" + path).toLowerCase(),
      amount,
      operation,
    };
    const display = getObject(modifier, "display");
    if (display) {
      const rawType = (getString(display, "type") ?? "reset").toLowerCase();
      output.display = rawType === "override"
        ? { type: "override", value: getString(display, "text") ?? "" }
        : { type: rawType === "hidden" ? "hidden" : "default" };
    }
    converted.push(output);
  }
  return converted.length > 0 ? converted : undefined;
}

function convertPersistentData(value: JsonValue | undefined, diagnostics: DiagnosticBag, source: string, item: string): JsonObject | undefined {
  const output: JsonObject = {};
  const exactlyRepresentable = new Set(["STRING", "INTEGER"]);
  for (const [index, entry] of sectionList(value).entries()) {
    const key = getString(entry, "key");
    const rawValue = getValue(entry, "value");
    const type = (getString(entry, "type") ?? "").toUpperCase();
    if (!key || rawValue === undefined || !type) {
      diagnostics.warning("PERSISTENT_DATA_INVALID", "Nexo ignores PersistentData entries without key, type, and value", { source, item, field: "PersistentData[" + index + "]", lossy: true });
      continue;
    }
    output[key.toLowerCase()] = deepClone(rawValue);
    if (!exactlyRepresentable.has(type)) diagnostics.warning("PERSISTENT_DATA_TYPE_APPROXIMATED", "CraftEngine pdc YAML cannot force Nexo's " + type + " scalar/array tag width; verify this value manually", { source, item, field: "PersistentData[" + index + "]", lossy: true });
  }
  return Object.keys(output).length > 0 ? output : undefined;
}

function mapRootData(config: JsonObject, diagnostics: DiagnosticBag, source: string, item: string, material: string): { data: JsonObject; componentItemModel?: string } {
  const data: JsonObject = {};
  if (typeof config.itemname === "string" && config.itemname.length > 0) data.item_name = config.itemname;
  if (typeof config.customname === "string" && config.customname.length > 0) data.custom_name = config.customname;
  if (Array.isArray(config.lore)) {
    const lore = config.lore.filter((entry): entry is string => typeof entry === "string");
    if (lore.length > 0) data.lore = lore;
  }
  const rootColor = typeof config.color === "string" ? nexoColor(config.color) : undefined;
  if (rootColor !== undefined) data.dyed_color = rootColor;
  if (Object.prototype.hasOwnProperty.call(config, "unbreakable")) data.unbreakable = config.unbreakable === true;
  if (isObject(config.Enchantments)) {
    const enchantments: JsonObject = {};
    for (const [rawId, rawLevel] of Object.entries(config.Enchantments)) {
      const id = normalizeLocation(rawId, diagnostics, { source, item, field: "Enchantments." + rawId });
      if (!id) continue;
      const level = bukkitInt(rawLevel);
      if (level < 1 || level > 255) {
        diagnostics.warning("ENCHANTMENT_LEVEL_CE_LIMIT", "Nexo does not clamp enchantment levels, but CraftEngine accepts only 1..255; the value was clamped", { source, item, field: "Enchantments." + rawId, lossy: true });
      }
      enchantments[id] = Math.max(1, Math.min(255, level));
    }
    if (Object.keys(enchantments).length > 0) data.enchantments = enchantments;
  }
  if (Object.prototype.hasOwnProperty.call(config, "max_durability")) {
    diagnostics.info("ROOT_MAX_DURABILITY_IGNORED", "Nexo 1.26 has no root max_durability parser; use Components.max_damage", { source, item, field: "max_durability" });
  }
  const attributes = convertAttributes(getValue(config, "AttributeModifiers"), diagnostics, source, item);
  if (attributes) data.attribute_modifiers = attributes;
  const pdc = convertPersistentData(getValue(config, "PersistentData"), diagnostics, source, item);
  if (pdc) data.pdc = pdc;
  const trimPattern = typeof config.trim_pattern === "string" ? normalizeLocation(config.trim_pattern, diagnostics, { source, item, field: "trim_pattern" }) : undefined;
  const trimMaterial = typeof config.trim_material === "string" ? normalizeLocation(config.trim_material, diagnostics, { source, item, field: "trim_material" }) : undefined;
  if (trimPattern) data.trim = { pattern: trimPattern, material: trimMaterial ?? "minecraft:redstone" };
  else if (trimMaterial) diagnostics.info("TRIM_MATERIAL_WITHOUT_PATTERN_IGNORED", "Nexo only emits an armor trim when trim_pattern resolves", { source, item, field: "trim_material" });
  const components = isObject(config.Components) ? config.Components : undefined;
  const componentItemModel = components ? mapComponents(components, data, diagnostics, source, item, material) : undefined;
  const unsetPrimary = components ? asStringList(getValue(components, "unset_components")) : [];
  const unset = unsetPrimary.length > 0 || !components ? unsetPrimary : asStringList(getValue(components, "unset_component"));
  const normalizedUnset = unset.map(normalizeComponentName).filter((value): value is string => value !== undefined);

  const effects = convertPotionEffects(getValue(config, "PotionEffects"), diagnostics, source, item);
  if (effects.length > 0 && !normalizedUnset.includes("potion_contents")) {
    const componentData = isObject(data.components) ? data.components : {};
    const potionContents: JsonObject = { custom_effects: effects };
    if (rootColor !== undefined) potionContents.custom_color = rootColor;
    componentData.potion_contents = potionContents;
    data.components = componentData;
  }
  // Nexo applies unset_components after every generated component, including
  // root PotionEffects, so keep this processor last in the emitted data map.
  if (normalizedUnset.length > 0) data.remove_components = normalizedUnset;
  if (getValue(config, "unset_components") !== undefined) {
    diagnostics.info("ROOT_UNSET_COMPONENTS_IGNORED", "Nexo 1.26 reads unset_components only inside Components", { source, item, field: "unset_components" });
  }
  if (getValue(config, "ItemFlags") !== undefined) {
    diagnostics.warning("ITEM_FLAGS_MANUAL", "Legacy Bukkit ItemFlags do not map one-to-one to modern tooltip_display", { source, item, field: "ItemFlags", lossy: true });
  }
  return { data, componentItemModel };
}

export function convertItem(
  item: ResolvedItem,
  options: ItemOptions,
  assignedCustomModelData: number | undefined,
  diagnostics: DiagnosticBag,
): ConvertedItem | undefined {
  if (item.template) return undefined;
  const targetId = options.namespace + ":" + item.id;
  const materialRaw = item.config.material;
  const matchedMaterial = matchBukkitMaterial(materialRaw);
  const material = matchedMaterial ?? "paper";
  if (!matchedMaterial && materialRaw !== undefined) {
    diagnostics.info("INVALID_MATERIAL_DEFAULTED", "Nexo Material.matchMaterial cannot resolve " + String(materialRaw) + "; PAPER was used", { source: item.source, item: item.id, field: "material" });
  }
  const pack = getObject(item.config, "Pack");
  const itemModelSection = getObject(item.config, "ItemModel");
  const modelContext = { source: item.source, item: item.id, diagnostics, modelAliases: options.modelAliases };
  const packInfo = readPackModel(pack, item.id, modelContext);
  const { data, componentItemModel } = mapRootData(item.config, diagnostics, item.source, item.id, material);
  const effectiveColor = typeof item.config.color === "string" && nexoColor(item.config.color) !== undefined ? item.config.color : undefined;
  const convertedModels = convertModels(packInfo, itemModelSection, material, effectiveColor, options.clientMode, modelContext);
  const ce: JsonObject = { material };
  if (Object.keys(data).length > 0) ce.data = data;
  if (convertedModels.model !== undefined) ce.model = convertedModels.model;
  if (convertedModels.legacyModel !== undefined) ce.legacy_model = convertedModels.legacyModel;
  if (assignedCustomModelData !== undefined) ce.custom_model_data = assignedCustomModelData;
  if (convertedModels.metadata) {
    ce.hand_animation_on_swap = convertedModels.metadata.handAnimationOnSwap;
    ce.oversized_in_gui = convertedModels.metadata.oversizedInGui;
    ce.swap_animation_scale = convertedModels.metadata.swapAnimationScale;
  }
  let modelPointer: string | undefined;
  if (options.clientMode !== "legacy") {
    modelPointer = componentItemModel;
    if (!modelPointer && convertedModels.generatedItemModel) {
      modelPointer = normalizeModelLocation(targetId, diagnostics, { source: item.source, item: item.id, field: "generated item_model" });
    }
    if (modelPointer) ce.item_model = modelPointer;
  } else if (componentItemModel) {
    diagnostics.warning("ITEM_MODEL_DROPPED_IN_LEGACY_MODE", "Components.item_model is unavailable to legacy clients", { source: item.source, item: item.id, field: "Components.item_model", lossy: true });
  }
  if (getValue(item.config, "crucible") !== undefined || getValue(item.config, "crucible_id") !== undefined || getValue(item.config, "mmoitem") !== undefined) {
    diagnostics.warning("EXTERNAL_ITEM_PROVIDER", "External item providers require a matching CraftEngine integration and were not copied automatically", { source: item.source, item: item.id, lossy: true });
  }
  return {
    sourceId: item.id,
    targetId,
    config: ce,
    modelPointer,
    baseModel: convertedModels.baseModel,
    semantics: {
      material_scope: material,
      item_model: modelPointer ?? null,
      custom_model_data: assignedCustomModelData ?? null,
      ...convertedModels.modelSemantics,
    },
  };
}
