import { basename, relative } from "node:path";
import { validateNamespace } from "./resource-location.js";

export interface NamespaceInference {
  namespace: string;
  evidence: string;
  candidates: string[];
}

function normalizeNamespace(raw: string): string | undefined {
  const normalized = raw.normalize("NFKC").trim().toLowerCase()
    .replace(/\.ya?ml$/i, "")
    .replace(/[^a-z0-9_.-]+/g, "_")
    .replace(/_+/g, "_")
    .replace(/^[_\-.]+|[_\-.]+$/g, "");
  return normalized && validateNamespace(normalized) ? normalized : undefined;
}

function tokenSubsequence(needle: string[], haystack: string[]): boolean {
  let index = 0;
  for (const token of haystack) if (token === needle[index]) index++;
  return index === needle.length;
}

function inferFromNexoItemPaths(paths: string[]): NamespaceInference | undefined {
  const normalizedPaths = paths.map((path) => path.replaceAll("\\", "/"));
  const basenames = Array.from(new Set(normalizedPaths
    .filter((path) => /\.ya?ml$/i.test(path) && !/(^|\/)templates?(\/|$)/i.test(path))
    .map((path) => normalizeNamespace(basename(path)))
    .filter((value): value is string => Boolean(value))
    .filter((value) => !["item", "items", "config", "configs", "categories", "template", "templates"].includes(value))));
  if (basenames.length === 1) {
    return { namespace: basenames[0]!, evidence: "Nexo item configuration filename", candidates: basenames };
  }
  if (basenames.length > 1) {
    const tokenized = new Map(basenames.map((value) => [value, value.split("_").filter(Boolean)]));
    const universal = basenames.filter((candidate) => {
      const tokens = tokenized.get(candidate)!;
      return tokens.length >= 2 && basenames.every((other) => tokenSubsequence(tokens, tokenized.get(other)!));
    }).sort((left, right) => tokenized.get(right)!.length - tokenized.get(left)!.length || left.localeCompare(right));
    if (universal.length === 1) {
      return { namespace: universal[0]!, evidence: "shared author name in Nexo item configuration filenames", candidates: basenames };
    }
  }

  const itemDirectories = new Set<string>();
  for (const path of normalizedPaths) {
    const match = /(?:^|\/)(?:items?|item)\/([^/]+)\//i.exec(path);
    const candidate = match ? normalizeNamespace(match[1]!) : undefined;
    if (candidate && !["template", "templates"].includes(candidate)) itemDirectories.add(candidate);
  }
  if (itemDirectories.size === 1) {
    const namespace = Array.from(itemDirectories)[0]!;
    return { namespace, evidence: "Nexo author item directory", candidates: basenames };
  }
  return undefined;
}

export function inferAuthorNamespaceFromBundlePaths(paths: string[], nexoRoot: string): NamespaceInference | undefined {
  const normalizedRoot = nexoRoot.replaceAll("\\", "/").replace(/^\.\/$|\/$/g, "");
  const normalizedPaths = paths.map((path) => path.replaceAll("\\", "/").replace(/^\.\//, ""));
  const explicit = new Map<string, Set<string>>();
  const add = (raw: string, source: string): void => {
    const namespace = normalizeNamespace(raw);
    if (!namespace) return;
    const sources = explicit.get(namespace) ?? new Set<string>();
    sources.add(source);
    explicit.set(namespace, sources);
  };
  for (const path of normalizedPaths) {
    const itemsAdder = /(?:^|\/)itemsadder\/contents\/([^/]+)\/(?:configs|resourcepack)(?:\/|$)/i.exec(path);
    if (itemsAdder) add(itemsAdder[1]!, "ItemsAdder contents namespace");
    const mythic = /(?:^|\/)mythicmobs\/packs\/([^/]+)\//i.exec(path);
    if (mythic) add(mythic[1]!, "MythicMobs pack namespace");
  }
  const rootPrefix = normalizedRoot && normalizedRoot !== "." ? normalizedRoot + "/" : "";
  const nexoItemPaths = normalizedPaths.filter((path) => path.startsWith(rootPrefix) && /(?:^|\/)(?:items?|item)\/.+\.ya?ml$/i.test(path));
  const nexoInference = inferFromNexoItemPaths(nexoItemPaths);
  if (explicit.size === 1) {
    const [namespace, sources] = Array.from(explicit.entries())[0]!;
    return { namespace, evidence: Array.from(sources).sort().join(" + "), candidates: Array.from(explicit.keys()) };
  }
  if (explicit.size > 1 && nexoInference && explicit.has(nexoInference.namespace)) {
    return {
      namespace: nexoInference.namespace,
      evidence: Array.from(explicit.get(nexoInference.namespace)!).sort().join(" + ") + " + " + nexoInference.evidence,
      candidates: Array.from(explicit.keys()).sort(),
    };
  }
  return nexoInference;
}

export function inferAuthorNamespaceFromNexoFiles(nexoRoot: string, files: string[]): NamespaceInference | undefined {
  return inferFromNexoItemPaths(files.map((file) => relative(nexoRoot, file)));
}
