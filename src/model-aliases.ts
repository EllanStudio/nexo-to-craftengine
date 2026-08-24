import { readdir } from "node:fs/promises";
import { join, relative } from "node:path";
import type { DiagnosticBag } from "./diagnostics.js";
import type { ResolvedItem } from "./items.js";
import { getObject, getString, getValue } from "./types.js";

const SINGLE_MODEL_KEYS = [
  "model", "blocking_model", "charged_model", "cast_model", "broken_model",
  "firework_model", "dyeable_model", "throwing_model",
] as const;
const LIST_MODEL_KEYS = ["pulling_models", "damaged_models", "composite_models"] as const;
const LOCATION = /^(?:([a-z0-9_.-]+):)?([a-z0-9/._-]+?)(?:\.json)?$/;

interface SourceReference {
  location: string;
  source: string;
  item: string;
  field: string;
}

function lookupLocation(raw: string): string | undefined {
  const normalized = raw.trim().replaceAll("\\", "/");
  const match = LOCATION.exec(normalized);
  if (!match) return undefined;
  return (match[1] ?? "minecraft") + ":" + match[2];
}

function editDistance(left: string, right: string): number {
  const previous = Array.from({ length: right.length + 1 }, (_, index) => index);
  for (let i = 1; i <= left.length; i++) {
    const current = [i];
    for (let j = 1; j <= right.length; j++) {
      current[j] = Math.min(
        (current[j - 1] ?? 0) + 1,
        (previous[j] ?? 0) + 1,
        (previous[j - 1] ?? 0) + (left[i - 1] === right[j - 1] ? 0 : 1),
      );
    }
    previous.splice(0, previous.length, ...current);
  }
  return previous[right.length] ?? Number.MAX_SAFE_INTEGER;
}

function commonPrefixRatio(left: string, right: string): number {
  let length = 0;
  while (length < left.length && length < right.length && left[length] === right[length]) length++;
  return length / Math.max(left.length, right.length, 1);
}

async function modelLocations(resourcePackRoot: string): Promise<Set<string>> {
  const assetsRoot = join(resourcePackRoot, "assets");
  const result = new Set<string>();
  const visit = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true }).catch(() => [])) {
      const child = join(directory, entry.name);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile() && entry.name.toLowerCase().endsWith(".json")) {
        const path = relative(assetsRoot, child).replaceAll("\\", "/");
        const match = /^([^/]+)\/models\/(.+)\.json$/i.exec(path);
        if (match) result.add(match[1] + ":" + match[2]);
      }
    }
  };
  await visit(assetsRoot);
  return result;
}

function sourceReferences(items: ResolvedItem[]): SourceReference[] {
  const result: SourceReference[] = [];
  for (const item of items) {
    if (item.template) continue;
    const pack = getObject(item.config, "Pack");
    if (!pack) continue;
    for (const key of SINGLE_MODEL_KEYS) {
      const raw = getString(pack, key);
      const location = raw ? lookupLocation(raw) : undefined;
      if (location) result.push({ location, source: item.source, item: item.id, field: "Pack." + key });
    }
    for (const key of LIST_MODEL_KEYS) {
      const raw = getValue(pack, key);
      const values = Array.isArray(raw) ? raw : typeof raw === "string" ? [raw] : [];
      for (const value of values) {
        const location = typeof value === "string" ? lookupLocation(value) : undefined;
        if (location) result.push({ location, source: item.source, item: item.id, field: "Pack." + key });
      }
    }
  }
  return result;
}

/**
 * Recover only a uniquely identifiable filename typo by pointing at an existing
 * model in the same namespace/directory. This never creates or renames assets.
 */
export async function discoverModelAliases(
  resourcePackRoot: string | undefined,
  items: ResolvedItem[],
  diagnostics: DiagnosticBag,
): Promise<ReadonlyMap<string, string>> {
  if (!resourcePackRoot) return new Map();
  const existing = await modelLocations(resourcePackRoot);
  const byDirectory = new Map<string, string[]>();
  for (const location of existing) {
    const slash = location.lastIndexOf("/");
    const directory = slash >= 0 ? location.slice(0, slash) : location.slice(0, location.indexOf(":") + 1);
    const values = byDirectory.get(directory) ?? [];
    values.push(location);
    byDirectory.set(directory, values);
  }
  const aliases = new Map<string, string>();
  const reported = new Set<string>();
  for (const reference of sourceReferences(items)) {
    if (existing.has(reference.location) || aliases.has(reference.location)) continue;
    const slash = reference.location.lastIndexOf("/");
    const directory = slash >= 0 ? reference.location.slice(0, slash) : reference.location.slice(0, reference.location.indexOf(":") + 1);
    const basename = reference.location.slice(slash + 1);
    if (basename.length < 12) continue;
    const stem = basename.slice(0, basename.lastIndexOf("_"));
    if (!stem) continue;
    const candidates = (byDirectory.get(directory) ?? []).map((location) => {
      const candidate = location.slice(location.lastIndexOf("/") + 1);
      const candidateStem = candidate.slice(0, candidate.lastIndexOf("_"));
      return { location, candidate, candidateStem, distance: editDistance(basename, candidate) };
    }).filter((entry) => entry.candidateStem === stem && entry.distance <= 2 && commonPrefixRatio(basename, entry.candidate) >= 0.75)
      .sort((left, right) => left.distance - right.distance || left.location.localeCompare(right.location));
    if (candidates.length === 0) continue;
    const best = candidates[0]!;
    if (candidates[1]?.distance === best.distance) continue;
    aliases.set(reference.location, best.location);
    if (!reported.has(reference.location)) {
      diagnostics.info(
        "MODEL_REFERENCE_TYPO_RECOVERED",
        "Missing model reference " + reference.location + " was redirected to the unique existing near-match " + best.location + "; no asset file was created",
        { source: reference.source, item: reference.item, field: reference.field },
      );
      reported.add(reference.location);
    }
  }
  return aliases;
}
