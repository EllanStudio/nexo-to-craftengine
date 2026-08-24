export type JsonScalar = string | number | boolean | null;
export type JsonValue = JsonScalar | JsonValue[] | JsonObject;
export interface JsonObject { [key: string]: JsonValue }

export function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function deepClone<T extends JsonValue>(value: T): T {
  return structuredClone(value);
}

export function findKey(object: JsonObject, wanted: string): string | undefined {
  const needle = wanted.toLowerCase();
  return Object.keys(object).find((key) => key.toLowerCase() === needle);
}

export function hasKey(object: JsonObject, wanted: string): boolean {
  return findKey(object, wanted) !== undefined;
}

export function getValue(object: JsonObject, wanted: string): JsonValue | undefined {
  const key = findKey(object, wanted);
  return key === undefined ? undefined : object[key];
}

export function getObject(object: JsonObject, wanted: string): JsonObject | undefined {
  const value = getValue(object, wanted);
  return isObject(value) ? value : undefined;
}

export function getString(object: JsonObject, wanted: string): string | undefined {
  const value = getValue(object, wanted);
  return typeof value === "string" ? value : undefined;
}

export function getBoolean(object: JsonObject, wanted: string, fallback: boolean): boolean {
  const value = getValue(object, wanted);
  return typeof value === "boolean" ? value : fallback;
}

export function getNumber(object: JsonObject, wanted: string): number | undefined {
  const value = getValue(object, wanted);
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function asStringList(value: JsonValue | undefined): string[] {
  if (typeof value === "string") return [value];
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string");
}

export function deepMerge(base: JsonObject, override: JsonObject): JsonObject {
  const result = deepClone(base);
  for (const [key, value] of Object.entries(override)) {
    const prior = result[key];
    if (isObject(prior) && isObject(value)) result[key] = deepMerge(prior, value);
    else result[key] = deepClone(value);
  }
  return result;
}

export function withoutKeys(object: JsonObject, names: readonly string[]): JsonObject {
  const denied = new Set(names.map((name) => name.toLowerCase()));
  return Object.fromEntries(Object.entries(object).filter(([key]) => !denied.has(key.toLowerCase()))) as JsonObject;
}

export function compactObject(entries: Array<[string, JsonValue | undefined]>): JsonObject {
  const result: JsonObject = {};
  for (const [key, value] of entries) if (value !== undefined) result[key] = value;
  return result;
}
