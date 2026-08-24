import { createHash } from "node:crypto";
import { deepClone, isObject, type JsonObject, type JsonValue } from "./types.js";

const ITEM_ID_ARGUMENT = "${__NAMESPACE__}:${__ID__}";
const SHIFT_ARGUMENT = "__nexo2ce_vertical_shift";
type Anchor = "ground" | "ceiling";

interface GridEntry {
  name: string;
  shift: number;
}

interface GridGroup {
  anchor: Anchor;
  entries: GridEntry[];
  toggleable: boolean;
}

export interface CompactedFurnitureDefinition {
  definition: JsonObject;
  templates: JsonObject;
}

function canonical(value: JsonValue): string {
  if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
  if (isObject(value)) {
    return "{" + Object.keys(value).sort().map((key) => JSON.stringify(key) + ":" + canonical(value[key]!)).join(",") + "}";
  }
  return JSON.stringify(value);
}

function digest(value: JsonValue): string {
  return createHash("sha256").update(canonical(value)).digest("hex").slice(0, 16);
}

function namespaceOf(id: string): string {
  const separator = id.indexOf(":");
  if (separator <= 0) throw new Error("CraftEngine furniture id must be namespaced: " + id);
  return id.slice(0, separator);
}

function parameterizeItem(value: JsonValue, targetId: string): JsonValue {
  if (typeof value === "string") return value === targetId ? ITEM_ID_ARGUMENT : value;
  if (Array.isArray(value)) return value.map((entry) => parameterizeItem(entry, targetId));
  if (!isObject(value)) return value;
  const result: JsonObject = {};
  for (const [key, entry] of Object.entries(value)) result[key] = parameterizeItem(entry, targetId);
  return result;
}

function registerTemplate(templates: JsonObject, id: string, value: JsonValue): void {
  const existing = templates[id];
  if (existing === undefined) {
    templates[id] = deepClone(value);
    return;
  }
  if (canonical(existing) !== canonical(value)) {
    throw new Error("Generated CraftEngine template id collision: " + id);
  }
}

function internTemplate(
  templates: JsonObject,
  namespace: string,
  targetId: string,
  kind: string,
  value: JsonValue,
): string {
  const parameterized = parameterizeItem(value, targetId);
  const id = namespace + ":_nexo2ce/furniture/" + kind + "/" + digest(parameterized);
  registerTemplate(templates, id, parameterized);
  return id;
}

function fixedTemplateId(namespace: string, path: string): string {
  return namespace + ":_nexo2ce/furniture/" + path;
}

function asTemplateReference(ids: string[]): JsonValue {
  return ids.length === 1 ? ids[0]! : ids;
}

function invocation(ids: string[], argumentsMap?: JsonObject, merges?: JsonValue): JsonObject {
  const result: JsonObject = { template: asTemplateReference(ids) };
  if (argumentsMap && Object.keys(argumentsMap).length > 0) result.arguments = argumentsMap;
  if (merges !== undefined) result.merges = merges;
  return result;
}

function rounded(value: number): number {
  return Number(value.toFixed(8));
}

function gridEntries(anchor: Anchor): GridEntry[] {
  const entries: GridEntry[] = [{ name: anchor, shift: 0 }];
  for (let sixteenth = 1; sixteenth < 16; sixteenth++) {
    const fraction = sixteenth / 16;
    entries.push({
      name: "_nexo_" + anchor + "_barrier_grid_" + sixteenth,
      shift: rounded(anchor === "ground" ? 1 - fraction : -fraction),
    });
  }
  return entries;
}

function completeGrid(variants: JsonObject, anchor: Anchor): GridEntry[] | undefined {
  const entries = gridEntries(anchor);
  return entries.every((entry) => isObject(variants[entry.name])) ? entries : undefined;
}

function parseVector(value: JsonValue | undefined): [number, number, number] | undefined {
  if (typeof value !== "string" && typeof value !== "number" && !Array.isArray(value)) return undefined;
  const parts = Array.isArray(value) ? value : String(value).replaceAll(" ", "").split(",");
  if (parts.length < 3) return undefined;
  const vector = [Number(parts[0]), Number(parts[1]), Number(parts[2])] as [number, number, number];
  return vector.every(Number.isFinite) ? vector : undefined;
}

function numberToken(value: number): string {
  return String(rounded(value));
}

function addElementOrigins(value: JsonValue): JsonValue {
  const clone = deepClone(value);
  if (!isObject(clone) || !Array.isArray(clone.elements)) return clone;
  for (const rawElement of clone.elements) {
    if (isObject(rawElement) && rawElement.position === undefined) rawElement.position = "0,0,0";
  }
  return clone;
}

interface ShiftTransformState {
  seed: string;
  index: number;
  arguments: JsonObject;
}

function shiftedVectorTemplate(value: JsonValue, state: ShiftTransformState): string | undefined {
  const vector = parseVector(value);
  if (!vector) return undefined;
  const argument = "__nexo2ce_" + state.seed + "_y_" + state.index++;
  state.arguments[argument] = {
    type: "expression",
    expression: "(${" + SHIFT_ARGUMENT + "})+(" + numberToken(vector[1]) + ")",
    value_type: "double",
  };
  return numberToken(vector[0]) + ",${" + argument + "}," + numberToken(vector[2]);
}

function shiftedSeatTemplate(value: string, state: ShiftTransformState): string {
  const match = /^(\S+)(.*)$/u.exec(value.trim());
  if (!match) return value;
  const position = shiftedVectorTemplate(match[1]!, state);
  return position === undefined ? value : position + match[2]!;
}

function makeShiftable(value: JsonValue, state: ShiftTransformState): JsonValue {
  if (Array.isArray(value)) return value.map((entry) => makeShiftable(entry, state));
  if (!isObject(value)) return value;
  const result: JsonObject = {};
  for (const [key, entry] of Object.entries(value)) {
    if ((key === "position" || key === "loot_spawn_offset") && entry !== undefined) {
      result[key] = shiftedVectorTemplate(entry, state) ?? makeShiftable(entry, state);
      continue;
    }
    if (key === "seats" && Array.isArray(entry)) {
      result[key] = entry.map((seat) => typeof seat === "string" ? shiftedSeatTemplate(seat, state) : makeShiftable(seat, state));
      continue;
    }
    result[key] = makeShiftable(entry, state);
  }
  return result;
}

function createShiftableTemplate(
  templates: JsonObject,
  namespace: string,
  targetId: string,
  kind: "variant" | "lights",
  source: JsonValue,
): string {
  const withOrigins = kind === "variant" ? addElementOrigins(source) : deepClone(source);
  const parameterized = parameterizeItem(withOrigins, targetId);
  const seed = digest(parameterized).slice(0, 10);
  const state: ShiftTransformState = { seed, index: 0, arguments: {} };
  const body = makeShiftable(parameterized, state);
  const bodyId = internTemplate(templates, namespace, targetId, kind + "-shift-body", body);
  const outer: JsonObject = { template: bodyId };
  if (Object.keys(state.arguments).length > 0) outer.arguments = state.arguments;
  return internTemplate(templates, namespace, targetId, kind + "-shift", outer);
}

function profileArgument(anchor: Anchor): string {
  return "__nexo2ce_" + anchor + "_profile_template";
}

function lightArgument(anchor: Anchor): string {
  return "__nexo2ce_" + anchor + "_light_template";
}

function ensureGridVariantsTemplate(templates: JsonObject, namespace: string, anchor: Anchor, unlit: boolean): string {
  const id = fixedTemplateId(namespace, "grid/" + anchor + (unlit ? "/unlit-variants" : "/variants"));
  const argument = profileArgument(anchor);
  const variants: JsonObject = {};
  for (const entry of gridEntries(anchor)) {
    variants[entry.name + (unlit ? "_unlit" : "")] = {
      template: "${" + argument + "}",
      arguments: { [SHIFT_ARGUMENT]: entry.shift },
    };
  }
  registerTemplate(templates, id, variants);
  return id;
}

function ensurePlaceFunctionsTemplate(templates: JsonObject, namespace: string, anchor: Anchor): string {
  const id = fixedTemplateId(namespace, "grid/" + anchor + "/place-functions");
  const functions: JsonObject[] = [];
  for (let sixteenth = 1; sixteenth < 16; sixteenth++) {
    const fraction = sixteenth / 16;
    const name = "_nexo_" + anchor + "_barrier_grid_" + sixteenth;
    functions.push({
      type: "set_furniture_variant",
      variant: name,
      conditions: [
        { type: "match_furniture_variant", variant: anchor },
        {
          type: "expression",
          expression: "ABS((<arg:position.y>-FLOOR(<arg:position.y>))-" + fraction + ")<0.00001",
        },
      ],
    });
  }
  registerTemplate(templates, id, functions);
  return id;
}

function ensureToggleCasesTemplate(templates: JsonObject, namespace: string, anchor: Anchor): string {
  const id = fixedTemplateId(namespace, "grid/" + anchor + "/toggle-cases");
  const cases: JsonObject[] = [];
  for (const entry of gridEntries(anchor)) {
    const unlit = entry.name + "_unlit";
    cases.push(
      { when: entry.name, functions: [{ type: "set_furniture_variant", variant: unlit }] },
      { when: unlit, functions: [{ type: "set_furniture_variant", variant: entry.name }] },
    );
  }
  registerTemplate(templates, id, cases);
  return id;
}

function ensureLightVariantsTemplate(templates: JsonObject, namespace: string, anchor: Anchor): string {
  const id = fixedTemplateId(namespace, "grid/" + anchor + "/light-variants");
  const argument = lightArgument(anchor);
  const variants: JsonObject = {};
  for (const entry of gridEntries(anchor)) {
    variants[entry.name] = {
      template: "${" + argument + "}",
      arguments: { [SHIFT_ARGUMENT]: entry.shift },
    };
  }
  registerTemplate(templates, id, variants);
  return id;
}

function internValueMap(
  templates: JsonObject,
  namespace: string,
  targetId: string,
  map: JsonObject,
  valueKind: string,
  mapKind: string,
): string | undefined {
  if (Object.keys(map).length === 0) return undefined;
  const references: JsonObject = {};
  for (const [key, value] of Object.entries(map)) {
    const valueId = internTemplate(templates, namespace, targetId, valueKind, value);
    references[key] = { template: valueId };
  }
  return internTemplate(templates, namespace, targetId, mapKind, references);
}

function compactVariants(
  definition: JsonObject,
  templates: JsonObject,
  namespace: string,
  targetId: string,
): GridGroup[] {
  if (!isObject(definition.variants)) return [];
  const remaining = deepClone(definition.variants);
  const groups: GridGroup[] = [];
  const templateIds: string[] = [];
  const argumentsMap: JsonObject = {};

  for (const anchor of ["ground", "ceiling"] as const) {
    const entries = completeGrid(remaining, anchor);
    if (!entries) continue;
    const source = remaining[anchor];
    if (!isObject(source)) continue;
    const profileTemplate = createShiftableTemplate(templates, namespace, targetId, "variant", source);
    const toggleable = entries.every((entry) => isObject(remaining[entry.name + "_unlit"]));
    templateIds.push(ensureGridVariantsTemplate(templates, namespace, anchor, false));
    argumentsMap[profileArgument(anchor)] = profileTemplate;
    for (const entry of entries) delete remaining[entry.name];
    if (toggleable) {
      templateIds.push(ensureGridVariantsTemplate(templates, namespace, anchor, true));
      for (const entry of entries) delete remaining[entry.name + "_unlit"];
    }
    groups.push({ anchor, entries, toggleable });
  }

  const remainingId = internValueMap(
    templates, namespace, targetId, remaining, "variant", "variant-map",
  );
  if (remainingId) templateIds.push(remainingId);
  if (templateIds.length > 0) definition.variants = invocation(templateIds, argumentsMap);
  return groups;
}

function compactPlaceFunctions(
  definition: JsonObject,
  templates: JsonObject,
  namespace: string,
  groups: GridGroup[],
): void {
  if (!Array.isArray(definition.events) || groups.length === 0) return;
  const generatedNames = new Set(groups.flatMap((group) => group.entries.slice(1).map((entry) => entry.name)));
  const templateIds = groups.map((group) => ensurePlaceFunctionsTemplate(templates, namespace, group.anchor));
  for (const event of definition.events) {
    if (!isObject(event) || event.on !== "place" || !Array.isArray(event.functions)) continue;
    const remaining = event.functions.filter((entry) => {
      return !(isObject(entry) && entry.type === "set_furniture_variant" && typeof entry.variant === "string" && generatedNames.has(entry.variant));
    });
    event.functions = invocation(templateIds, undefined, remaining.length > 0 ? remaining : undefined);
    return;
  }
}

function compactToggleCases(
  definition: JsonObject,
  templates: JsonObject,
  namespace: string,
  groups: GridGroup[],
): void {
  const toggleGroups = groups.filter((group) => group.toggleable);
  if (!Array.isArray(definition.events) || toggleGroups.length === 0) return;
  const generatedNames = new Set(toggleGroups.flatMap((group) => group.entries.flatMap((entry) => [entry.name, entry.name + "_unlit"])));
  const templateIds = toggleGroups.map((group) => ensureToggleCasesTemplate(templates, namespace, group.anchor));
  for (const event of definition.events) {
    if (!isObject(event) || event.on !== "right_click" || !Array.isArray(event.functions)) continue;
    for (const fn of event.functions) {
      if (!isObject(fn) || fn.type !== "when" || fn.source !== "<arg:furniture.variant>" || !Array.isArray(fn.cases)) continue;
      const remaining = fn.cases.filter((entry) => !(isObject(entry) && typeof entry.when === "string" && generatedNames.has(entry.when)));
      fn.cases = invocation(templateIds, undefined, remaining.length > 0 ? remaining : undefined);
      return;
    }
  }
}

function compactLightVariants(
  definition: JsonObject,
  templates: JsonObject,
  namespace: string,
  targetId: string,
  groups: GridGroup[],
): void {
  if (!Array.isArray(definition.behaviors)) return;
  for (const behavior of definition.behaviors) {
    if (!isObject(behavior) || behavior.type !== "glowing_furniture" || !isObject(behavior.variants)) continue;
    const remaining = deepClone(behavior.variants);
    const templateIds: string[] = [];
    const argumentsMap: JsonObject = {};
    for (const group of groups) {
      const source = remaining[group.anchor];
      if (!Array.isArray(source)) continue;
      const lightTemplate = createShiftableTemplate(templates, namespace, targetId, "lights", source);
      templateIds.push(ensureLightVariantsTemplate(templates, namespace, group.anchor));
      argumentsMap[lightArgument(group.anchor)] = lightTemplate;
      for (const entry of group.entries) delete remaining[entry.name];
    }
    const remainingId = internValueMap(
      templates, namespace, targetId, remaining, "light-list", "light-variant-map",
    );
    if (remainingId) templateIds.push(remainingId);
    if (templateIds.length > 0) behavior.variants = invocation(templateIds, argumentsMap);
  }
}

/**
 * Re-encodes a fully concrete furniture definition with CraftEngine 26.8's
 * recursive template system. The plugin expands this back to equivalent native
 * variants before the furniture parser sees it; no companion runtime is used.
 */
export function compactFurnitureDefinition(definition: JsonObject, targetId: string): CompactedFurnitureDefinition {
  const namespace = namespaceOf(targetId);
  const templates: JsonObject = {};
  const compact = deepClone(definition);
  const groups = compactVariants(compact, templates, namespace, targetId);
  compactPlaceFunctions(compact, templates, namespace, groups);
  compactToggleCases(compact, templates, namespace, groups);
  compactLightVariants(compact, templates, namespace, targetId, groups);
  const familyId = internTemplate(templates, namespace, targetId, "family", compact);
  return { definition: { template: familyId }, templates };
}

export function mergeFurnitureTemplates(target: JsonObject, source: JsonObject): void {
  for (const [id, value] of Object.entries(source)) registerTemplate(target, id, value);
}
