import { deepClone, isObject, type JsonObject, type JsonValue } from "../src/types.js";

type ExpressionArgument = { kind: "expression"; expression: string };
type Argument = JsonValue | ExpressionArgument;
type Arguments = Map<string, Argument>;

function isExpressionArgument(value: Argument): value is ExpressionArgument {
  return isObject(value) && value.kind === "expression" && typeof value.expression === "string";
}

function evaluateExpression(expression: string): number {
  const compact = expression.replaceAll(" ", "");
  const match = /^\((-?(?:\d+(?:\.\d*)?|\.\d+))\)\+\((-?(?:\d+(?:\.\d*)?|\.\d+))\)$/u.exec(compact);
  if (!match) throw new Error("Unsupported generated CraftEngine expression in test expander: " + expression);
  // EvalEx evaluates decimal literals through BigDecimal before CE formats the
  // requested double. Round our binary JS addition to the converter's eight
  // decimal coordinate precision so this helper matches that runtime path.
  return Number((Number(match[1]) + Number(match[2])).toFixed(8));
}

function argumentValue(argument: Argument): JsonValue {
  return isExpressionArgument(argument) ? evaluateExpression(argument.expression) : deepClone(argument);
}

function replaceArguments(value: string, argumentsMap: Arguments): JsonValue {
  const exact = /^\$\{([^}]+)\}$/u.exec(value);
  if (exact) {
    const argument = argumentsMap.get(exact[1]!);
    if (argument === undefined) throw new Error("Missing generated CraftEngine template argument: " + exact[1]);
    return argumentValue(argument);
  }
  return value.replace(/\$\{([^}]+)\}/gu, (_whole, name: string) => {
    const argument = argumentsMap.get(name);
    if (argument === undefined) throw new Error("Missing generated CraftEngine template argument: " + name);
    const resolved = argumentValue(argument);
    return typeof resolved === "string" || typeof resolved === "number" || typeof resolved === "boolean"
      ? String(resolved)
      : JSON.stringify(resolved);
  });
}

function deepMerge(base: JsonValue, incoming: JsonValue): JsonValue {
  if (Array.isArray(base) && Array.isArray(incoming)) return [...deepClone(base), ...deepClone(incoming)];
  if (isObject(base) && isObject(incoming)) {
    const result = deepClone(base);
    for (const [key, value] of Object.entries(incoming)) {
      result[key] = result[key] === undefined ? deepClone(value) : deepMerge(result[key]!, value);
    }
    return result;
  }
  return deepClone(incoming);
}

function typedArgument(value: JsonValue): Argument {
  if (isObject(value) && value.type === "expression" && typeof value.expression === "string") {
    return { kind: "expression", expression: value.expression };
  }
  return deepClone(value);
}

function mergedArguments(raw: JsonObject | undefined, parent: Arguments, templates: JsonObject): Arguments {
  const result = new Map(parent);
  if (!raw) return result;
  for (const [name, value] of Object.entries(raw)) {
    if (result.has(name)) continue;
    const processed = expandNode(value, result, templates);
    result.set(name, typedArgument(processed));
  }
  return result;
}

function templateIds(value: JsonValue, argumentsMap: Arguments): string[] {
  const list = Array.isArray(value) ? value : [value];
  return list.map((entry) => {
    if (typeof entry !== "string") throw new Error("Generated CraftEngine template id is not a string");
    const resolved = replaceArguments(entry, argumentsMap);
    if (typeof resolved !== "string") throw new Error("Generated CraftEngine template id did not resolve to a string");
    return resolved;
  });
}

function expandTemplateMap(input: JsonObject, parent: Arguments, templates: JsonObject): JsonValue {
  const rawArguments = isObject(input.arguments) ? input.arguments : undefined;
  const argumentsMap = mergedArguments(rawArguments, parent, templates);
  const rawTemplate = input.template ?? input.templates;
  if (rawTemplate === undefined) throw new Error("Template map has no template id");
  const ids = templateIds(rawTemplate, argumentsMap);
  let result: JsonValue | undefined;
  for (const id of ids) {
    const template = templates[id];
    if (template === undefined) throw new Error("Unknown generated CraftEngine template: " + id);
    const expanded = expandNode(template, argumentsMap, templates);
    result = result === undefined ? expanded : deepMerge(result, expanded);
  }
  if (result === undefined) throw new Error("Generated CraftEngine template list was empty");

  const ordinary: JsonObject = {};
  for (const [key, value] of Object.entries(input)) {
    if (["template", "templates", "arguments", "merges", "overrides"].includes(key)) continue;
    const resolvedKey = replaceArguments(key, argumentsMap);
    if (typeof resolvedKey !== "string") throw new Error("Generated template key did not resolve to a string");
    ordinary[resolvedKey] = expandNode(value, argumentsMap, templates);
  }
  if (Object.keys(ordinary).length > 0) result = deepMerge(result, ordinary);
  if (input.merges !== undefined) result = deepMerge(result, expandNode(input.merges, argumentsMap, templates));
  if (input.overrides !== undefined) {
    const overrides = expandNode(input.overrides, argumentsMap, templates);
    if (Array.isArray(result) && Array.isArray(overrides)) result = overrides;
    else if (isObject(result) && isObject(overrides)) {
      for (const [key, value] of Object.entries(overrides)) result[key] = value;
    } else result = overrides;
  }
  return result;
}

function expandNode(value: JsonValue, argumentsMap: Arguments, templates: JsonObject): JsonValue {
  if (typeof value === "string") return replaceArguments(value, argumentsMap);
  if (Array.isArray(value)) return value.map((entry) => expandNode(entry, argumentsMap, templates));
  if (!isObject(value)) return value;
  if (value.template !== undefined || value.templates !== undefined) return expandTemplateMap(value, argumentsMap, templates);
  const result: JsonObject = {};
  for (const [key, entry] of Object.entries(value)) {
    const resolvedKey = replaceArguments(key, argumentsMap);
    if (typeof resolvedKey !== "string") throw new Error("Generated template key did not resolve to a string");
    result[resolvedKey] = expandNode(entry, argumentsMap, templates);
  }
  return result;
}

export function expandCraftEngineTemplateEntry(entry: JsonObject, templates: JsonObject, id: string): JsonObject {
  const separator = id.indexOf(":");
  const argumentsMap: Arguments = new Map([
    ["__NAMESPACE__", id.slice(0, separator)],
    ["__ID__", id.slice(separator + 1)],
  ]);
  const expanded = expandNode(entry, argumentsMap, templates);
  if (!isObject(expanded)) throw new Error("Expanded CraftEngine resource is not a map: " + id);
  return expanded;
}
