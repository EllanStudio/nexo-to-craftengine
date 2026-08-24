import type { DiagnosticBag } from "./diagnostics.js";
import { normalizeLocation } from "./resource-location.js";
import { getNumber, getObject, getString, getValue, isObject, type JsonObject, type JsonValue } from "./types.js";

export type NexoRecipeType = "shaped" | "shapeless" | "furnace" | "blasting" | "smoking" | "campfire" | "stonecutting" | "brewing";

interface RecipeContext {
  namespace: string;
  source: string;
  id: string;
  diagnostics: DiagnosticBag;
}

function detail(context: RecipeContext, field: string, lossy = false): { source: string; item: string; field: string; lossy: boolean } {
  return { source: context.source, item: context.id, field, lossy };
}

function normalizeItem(value: string, context: RecipeContext, field: string, custom: boolean): string | undefined {
  return normalizeLocation(value.toLowerCase(), context.diagnostics, detail(context, field), [], custom ? context.namespace : "minecraft");
}

function choice(section: JsonObject, context: RecipeContext, field: string): string | undefined {
  const nexo = getString(section, "nexo_item");
  if (nexo) return normalizeItem(nexo, context, field + ".nexo_item", true);
  if (getString(section, "crucible_item") || getString(section, "mmoitems_id")) {
    context.diagnostics.warning("RECIPE_EXACT_EXTERNAL_CHOICE", "Crucible/MMOItems ExactChoice has priority in Nexo and cannot be reconstructed as a plain CraftEngine item id", detail(context, field, true));
    return undefined;
  }
  const minecraft = getString(section, "minecraft_type");
  if (minecraft) return normalizeItem(minecraft, context, field + ".minecraft_type", false);
  const tag = getString(section, "tag");
  if (tag) {
    const location = normalizeItem(tag.replace(/^#/, ""), context, field + ".tag", false);
    return location ? "#" + location : undefined;
  }
  if (getValue(section, "nexo_tag") !== undefined) {
    context.diagnostics.warning("RECIPE_NEXO_TAG_UNEXPANDED", "Nexo expands nexo_tag into multiple recipes before loading; tag definitions are required for an exact conversion", detail(context, field, true));
    return undefined;
  }
  if (getValue(section, "minecraft_item") !== undefined) {
    context.diagnostics.warning("RECIPE_EXACT_SERIALIZED_ITEM", "A serialized Bukkit ItemStack ExactChoice cannot be reconstructed as a plain CraftEngine item id", detail(context, field, true));
    return undefined;
  }
  context.diagnostics.error("RECIPE_CHOICE_MISSING", "Recipe choice has no supported nexo_item, minecraft_type, or tag", detail(context, field));
  return undefined;
}

function result(section: JsonObject | undefined, context: RecipeContext): JsonObject | undefined {
  if (!section) {
    context.diagnostics.error("RECIPE_RESULT_MISSING", "Recipe has no result section", detail(context, "result"));
    return undefined;
  }
  const id = choice(section, context, "result");
  if (!id || id.startsWith("#")) return undefined;
  return { id, count: getNumber(section, "amount") ?? 1 };
}

function category(section: JsonObject): string | undefined {
  const value = getString(section, "category");
  return value?.toLowerCase();
}

function common(section: JsonObject, context: RecipeContext): JsonObject | undefined {
  const output = result(getObject(section, "result"), context);
  if (!output) return undefined;
  const converted: JsonObject = { result: output };
  const group = getString(section, "group");
  if (group !== undefined) converted.group = group;
  const cat = category(section);
  if (cat) converted.category = cat;
  if (getString(section, "permission")) context.diagnostics.warning("RECIPE_PERMISSION_MANUAL", "Nexo recipe permission has no direct built-in CraftEngine recipe field", detail(context, "permission", true));
  return converted;
}

export function convertRecipe(type: NexoRecipeType, id: string, section: JsonObject, namespace: string, diagnostics: DiagnosticBag, source: string): JsonObject | undefined {
  const context: RecipeContext = { namespace, source, id, diagnostics };
  const converted = common(section, context);
  if (!converted) return undefined;
  const typeMap: Record<NexoRecipeType, string> = {
    shaped: "shaped", shapeless: "shapeless", furnace: "smelting", blasting: "blasting", smoking: "smoking",
    campfire: "campfire_cooking", stonecutting: "stonecutting", brewing: "brewing",
  };
  converted.type = typeMap[type];
  if (type === "shaped") {
    const shape = getValue(section, "shape");
    if (!Array.isArray(shape) || !shape.every((entry) => typeof entry === "string")) {
      diagnostics.error("SHAPED_PATTERN_INVALID", "Nexo shaped recipe shape must be a string list", detail(context, "shape"));
      return undefined;
    }
    converted.pattern = shape;
    const ingredients = getObject(section, "ingredients");
    if (!ingredients) return undefined;
    const mapped: JsonObject = {};
    for (const [symbol, rawChoice] of Object.entries(ingredients)) {
      if (!isObject(rawChoice)) continue;
      const value = choice(rawChoice, context, "ingredients." + symbol);
      if (value) mapped[symbol.charAt(0)] = value;
    }
    const usedSymbols = new Set(shape.join("").split("").filter((symbol) => symbol !== " "));
    const missingSymbols = [...usedSymbols].filter((symbol) => mapped[symbol] === undefined);
    if (missingSymbols.length > 0) {
      diagnostics.error("SHAPED_INGREDIENT_MISSING", "CraftEngine rejects pattern symbols without ingredient mappings: " + missingSymbols.join(", "), detail(context, "ingredients"));
      return undefined;
    }
    converted.ingredients = mapped;
  } else if (type === "shapeless") {
    const raw = getValue(section, "ingredients");
    const list: JsonValue[] = [];
    const entries: JsonValue[] = Array.isArray(raw) ? raw : isObject(raw) ? Object.values(raw) : [];
    for (const [index, entry] of entries.entries()) {
      if (!isObject(entry)) continue;
      const value = choice(entry, context, "ingredients." + index);
      if (!value) continue;
      const amount = Math.max(1, Math.min(9, Math.trunc(getNumber(entry, "amount") ?? 1)));
      for (let count = 0; count < amount; count++) list.push(value);
    }
    converted.ingredients = list;
  } else if (["furnace", "blasting", "smoking", "campfire"].includes(type)) {
    const input = getObject(section, "input");
    if (!input) return undefined;
    const ingredient = choice(input, context, "input");
    if (!ingredient) return undefined;
    converted.ingredient = ingredient;
    converted.experience = getNumber(section, "experience") ?? 0;
    converted.time = getNumber(section, "cookingTime") ?? 0;
  } else if (type === "stonecutting") {
    const input = getObject(section, "input");
    if (!input) return undefined;
    const ingredient = choice(input, context, "input");
    if (!ingredient) return undefined;
    converted.ingredient = ingredient;
    delete converted.category;
  } else if (type === "brewing") {
    const input = getObject(section, "input");
    const reagent = getObject(section, "ingredient");
    if (!input || !reagent) return undefined;
    const container = choice(input, context, "input");
    const ingredient = choice(reagent, context, "ingredient");
    if (!container || !ingredient) return undefined;
    converted.container = container;
    converted.ingredient = ingredient;
    delete converted.group;
    delete converted.category;
  }
  return converted;
}
