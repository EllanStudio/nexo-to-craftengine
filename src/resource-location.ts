import { join } from "node:path";
import type { DiagnosticBag } from "./diagnostics.js";

const NAMESPACE = /^[a-z0-9_.-]+$/;
const RESOURCE_PATH = /^[a-z0-9/._-]+$/;
const ITEM_PATH = /^[a-z0-9/._-]+$/;

export interface LocationDetails {
  source?: string;
  item?: string;
  field?: string;
}

export function stripKnownExtension(value: string, extensions: readonly string[]): string {
  const lower = value.toLowerCase();
  for (const extension of extensions) {
    if (lower.endsWith(extension.toLowerCase())) return value.slice(0, -extension.length);
  }
  return value;
}

export function normalizeLocation(
  input: string,
  diagnostics: DiagnosticBag,
  details: LocationDetails,
  extensions: readonly string[] = [],
  defaultNamespace = "minecraft",
): string | undefined {
  let value = input.trim().replaceAll("\\", "/");
  value = stripKnownExtension(value, extensions);
  const separator = value.indexOf(":");
  const namespace = separator >= 0 ? value.slice(0, separator) : defaultNamespace;
  const path = separator >= 0 ? value.slice(separator + 1) : value;
  if (!NAMESPACE.test(namespace) || !RESOURCE_PATH.test(path) || path.startsWith("/") || path.split("/").includes("..")) {
    diagnostics.error("INVALID_RESOURCE_LOCATION", "Invalid Minecraft resource location: " + input, details);
    return undefined;
  }
  return namespace + ":" + path;
}

export function normalizeModelLocation(input: string, diagnostics: DiagnosticBag, details: LocationDetails): string | undefined {
  return normalizeLocation(input, diagnostics, details, [".json"]);
}

export function normalizeTextureLocation(input: string, diagnostics: DiagnosticBag, details: LocationDetails): string | undefined {
  if (input.startsWith("#")) return input;
  return normalizeLocation(input, diagnostics, details, [".png"]);
}

export function normalizeSoundLocation(input: string, diagnostics: DiagnosticBag, details: LocationDetails): string | undefined {
  return normalizeLocation(input, diagnostics, details, [".ogg"]);
}

export function normalizeItemPath(input: string, diagnostics: DiagnosticBag, details: LocationDetails): string | undefined {
  const value = input.trim().toLowerCase().replaceAll(" ", "_");
  if (!ITEM_PATH.test(value) || value.startsWith("/") || value.split("/").includes("..")) {
    diagnostics.error("INVALID_ITEM_ID", "Invalid item id: " + input, details);
    return undefined;
  }
  if (value !== input) diagnostics.warning("ITEM_ID_NORMALIZED", "Item id normalized from " + input + " to " + value, { ...details, lossy: true });
  return value;
}

export function validateNamespace(namespace: string): boolean {
  return NAMESPACE.test(namespace);
}

export function splitLocation(location: string): [string, string] {
  const separator = location.indexOf(":");
  return [location.slice(0, separator), location.slice(separator + 1)];
}

export function assetFile(resourceRoot: string, category: "models" | "textures" | "items" | "sounds" | "font", location: string, extension: string): string {
  const [namespace, path] = splitLocation(location);
  return join(resourceRoot, "assets", namespace, category, path + extension);
}

export function minimalLocation(location: string): string {
  return location.startsWith("minecraft:") ? location.slice("minecraft:".length) : location;
}

export function minecraftKey(value: string): string {
  return value.startsWith("minecraft:") ? value.slice("minecraft:".length) : value;
}
