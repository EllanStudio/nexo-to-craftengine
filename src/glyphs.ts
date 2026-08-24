import { readdir } from "node:fs/promises";
import { basename, extname, join } from "node:path";
import type { DiagnosticBag } from "./diagnostics.js";
import { loadYaml } from "./io.js";
import { normalizeLocation, normalizeTextureLocation } from "./resource-location.js";
import { asStringList, getBoolean, getNumber, getString, getValue, isObject, type JsonObject, type JsonValue } from "./types.js";

export interface GlyphEntry {
  sourceId: string;
  targetId: string;
  font: string;
  texture?: string;
  /** Logical Nexo rows. Reference glyphs deliberately have one logical row. */
  chars: string[];
  /** Column count of the underlying bitmap image. */
  columns: number;
  /** Zero-based offset into the underlying bitmap image. */
  startIndex: number;
  permission?: string;
}

export interface GlyphConversion {
  images: JsonObject;
  entries: Record<string, GlyphEntry>;
  sourceFiles: string[];
}

interface RawGlyph {
  id: string;
  section: JsonObject;
  source: string;
}

async function yamlFiles(directory: string): Promise<string[]> {
  const output: string[] = [];
  const visit = async (path: string): Promise<void> => {
    let entries;
    try { entries = await readdir(path, { withFileTypes: true }); } catch { return; }
    for (const entry of entries) {
      const child = join(path, entry.name);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile() && [".yml", ".yaml"].includes(extname(entry.name).toLowerCase())) output.push(child);
    }
  };
  await visit(directory);
  return output.sort((a, b) => basename(a, extname(a)).localeCompare(basename(b, extname(b))) || a.localeCompare(b));
}

function stringCodePoints(value: string): number[] {
  return Array.from(value, (character) => character.codePointAt(0)!);
}

function allocateChars(rows: number, columns: number, used: Set<number>): string[] {
  let candidate = 42000;
  const values: string[] = [];
  for (let index = 0; index < rows * columns; index++) {
    while (used.has(candidate) || (candidate >= 0xd800 && candidate <= 0xdfff)) candidate++;
    if (candidate > 0x10ffff) throw new Error("Unicode code-point space exhausted during glyph allocation");
    used.add(candidate);
    values.push(String.fromCodePoint(candidate++));
  }
  const result: string[] = [];
  for (let row = 0; row < rows; row++) result.push(values.slice(row * columns, (row + 1) * columns).join(""));
  return result;
}

function gridColumns(chars: string[]): number {
  return chars.length > 0 ? stringCodePoints(chars[0] ?? "").length : 0;
}

function coordinate(zeroBasedIndex: number, columns: number): [number, number] {
  const safe = Math.max(0, zeroBasedIndex);
  return [Math.floor(safe / Math.max(1, columns)), safe % Math.max(1, columns)];
}

function glyphCount(entry: GlyphEntry): number {
  return entry.chars.reduce((sum, row) => sum + stringCodePoints(row).length, 0);
}

function entryAliases(entries: Record<string, GlyphEntry>, id: string, entry: GlyphEntry): void {
  entries[id] = entry;
  entries[id.toLowerCase()] = entry;
}

function configuredPermission(section: JsonObject, id: string, fallback?: string): string | undefined {
  const value = getString(section, "permission") ?? fallback?.replaceAll("<glyphid>", id);
  return value && value.length > 0 ? value : undefined;
}

function emitAuxiliaryDiagnostics(glyph: RawGlyph, permission: string | undefined, diagnostics: DiagnosticBag): void {
  if (permission) diagnostics.warning("GLYPH_PERMISSION_MANUAL", "CraftEngine image tags do not enforce Nexo glyph permission automatically", { source: glyph.source, item: glyph.id, field: "permission", lossy: true });
  if (getValue(glyph.section, "placeholder") !== undefined || getBoolean(glyph.section, "is_emoji", false) || getBoolean(glyph.section, "tabcomplete", false)) {
    diagnostics.warning("GLYPH_PLACEHOLDER_MANUAL", "Nexo glyph placeholder, emoji, and tab-completion behavior needs a CraftEngine emoji/PAPI policy", { source: glyph.source, item: glyph.id, field: "placeholder", lossy: true });
  }
  if (getValue(glyph.section, "default_shadow_color") !== undefined) {
    diagnostics.warning("GLYPH_SHADOW_MANUAL", "Nexo glyph default shadow color has no image-level CraftEngine equivalent", { source: glyph.source, item: glyph.id, field: "default_shadow_color", lossy: true });
  }
}

function parseReferenceRange(value: JsonValue | undefined): [number, number] | undefined {
  const text = typeof value === "number" ? String(Math.trunc(value)) : typeof value === "string" ? value : "";
  if (!/^-?\d+(?:\.\.-?\d+)?$/.test(text)) return undefined;
  const [firstRaw, lastRaw] = text.split("..");
  const first = Number(firstRaw);
  const parsedLast = Number(lastRaw ?? firstRaw);
  return [first, Math.max(first, parsedLast)];
}

export async function convertGlyphs(sourceRoot: string, namespace: string, diagnostics: DiagnosticBag, defaultFont = "nexo:default", defaultPermission = "nexo.glyphs.<glyphid>"): Promise<GlyphConversion> {
  const sourceFiles = await yamlFiles(join(sourceRoot, "glyphs"));
  const raw: RawGlyph[] = [];
  const seen = new Set<string>();
  const seenFolded = new Set<string>();
  for (const file of sourceFiles) {
    const loaded = await loadYaml(file, diagnostics);
    if (!isObject(loaded)) continue;
    for (const [id, value] of Object.entries(loaded)) {
      if (!isObject(value)) continue;
      if (seen.has(id)) diagnostics.error("DUPLICATE_GLYPH_ID", "Duplicate Nexo glyph id: " + id, { source: file, item: id });
      else if (seenFolded.has(id.toLowerCase())) diagnostics.error("DUPLICATE_GLYPH_ID_CASE", "Nexo glyph ids differ only by case and collide in CraftEngine: " + id, { source: file, item: id });
      seen.add(id);
      seenFolded.add(id.toLowerCase());
      raw.push({ id, section: value, source: file });
    }
  }

  // CE, Minecraft, and Nexo allocate glyph code points independently inside
  // each font. Resolve fonts before reservation so equal chars in different
  // fonts do not conflict and each font's automatic sequence starts at 42000.
  const fontByGlyph = new Map<RawGlyph, string>();
  const usedByFont = new Map<string, Set<number>>();
  const ownersByFont = new Map<string, Map<number, string>>();
  for (const glyph of raw) {
    if (getString(glyph.section, "reference")) continue;
    const fontRaw = getString(glyph.section, "font") ?? defaultFont;
    const font = normalizeLocation(fontRaw, diagnostics, { source: glyph.source, item: glyph.id, field: "font" }) ?? defaultFont;
    fontByGlyph.set(glyph, font);
    const used = usedByFont.get(font) ?? new Set<number>();
    const owners = ownersByFont.get(font) ?? new Map<number, string>();
    usedByFont.set(font, used);
    ownersByFont.set(font, owners);
    for (const row of asStringList(getValue(glyph.section, "char"))) {
      for (const code of stringCodePoints(row)) {
        const owner = owners.get(code);
        if (owner) diagnostics.error("GLYPH_CHAR_CONFLICT", "Glyph char is assigned more than once in font " + font + " (also used by " + owner + ")", { source: glyph.source, item: glyph.id, field: "char", lossy: true });
        else owners.set(code, glyph.id);
        used.add(code);
      }
    }
  }

  const images: JsonObject = {};
  const entries: Record<string, GlyphEntry> = {};
  const references: RawGlyph[] = [];
  for (const glyph of raw) {
    if (getString(glyph.section, "reference")) {
      references.push(glyph);
      continue;
    }
    if (getValue(glyph.section, "gif") !== undefined) {
      diagnostics.warning("ANIMATED_GLYPH_UNSUPPORTED", "Nexo animated glyphs use sprite/shader runtime behavior and were not converted", { source: glyph.source, item: glyph.id, field: "gif", lossy: true });
      continue;
    }

    const font = fontByGlyph.get(glyph) ?? defaultFont;
    const used = usedByFont.get(font) ?? new Set<number>();
    usedByFont.set(font, used);
    const rawRows = Math.trunc(getNumber(glyph.section, "rows") ?? 1);
    const rawColumns = Math.trunc(getNumber(glyph.section, "columns") ?? 1);
    if (rawRows <= 0 || rawColumns <= 0) {
      diagnostics.error("GLYPH_GRID_SIZE_INVALID", "Nexo glyph rows and columns must both be positive", { source: glyph.source, item: glyph.id, field: "rows", lossy: true });
      continue;
    }
    let chars = asStringList(getValue(glyph.section, "char"));
    if (chars.length === 0) chars = allocateChars(rawRows, rawColumns, used);
    const columnsFromChars = gridColumns(chars);
    if (columnsFromChars === 0 || chars.some((row) => stringCodePoints(row).length !== columnsFromChars)) {
      diagnostics.error("GLYPH_CHAR_GRID_INVALID", "Every Nexo glyph char row must have the same non-zero Unicode code-point width", { source: glyph.source, item: glyph.id, field: "char", lossy: true });
      continue;
    }

    const texture = normalizeTextureLocation(getString(glyph.section, "texture") ?? "minecraft:required/exit_icon", diagnostics, { source: glyph.source, item: glyph.id, field: "texture" });
    const targetId = normalizeLocation(namespace + ":" + glyph.id.toLowerCase(), diagnostics, { source: glyph.source, item: glyph.id, field: "id" });
    if (!texture || !targetId) continue;
    const height = Math.trunc(getNumber(glyph.section, "height") ?? 8);
    if (height <= 0) {
      diagnostics.error("GLYPH_HEIGHT_INVALID", "Nexo glyph height must be positive for CraftEngine and Minecraft", { source: glyph.source, item: glyph.id, field: "height", lossy: true });
      continue;
    }
    // Nexo creates the bitmap provider with min(ascent, height).
    const ascent = Math.min(Math.trunc(getNumber(glyph.section, "ascent") ?? 8), height);
    images[targetId] = { file: texture, font, height, ascent, chars };
    const permission = configuredPermission(glyph.section, glyph.id, defaultPermission);
    const entry: GlyphEntry = { sourceId: glyph.id, targetId, font, texture, chars, columns: columnsFromChars, startIndex: 0, permission };
    entryAliases(entries, glyph.id, entry);
    emitAuxiliaryDiagnostics(glyph, permission, diagnostics);
  }

  // Resolve references only after ordinary glyphs. Iteration supports reference
  // chains without making YAML file order observable; cycles remain invalid.
  let pending = references;
  while (pending.length > 0) {
    const next: RawGlyph[] = [];
    let progress = false;
    for (const glyph of pending) {
      const reference = getString(glyph.section, "reference")!;
      const sourceEntry = entries[reference] ?? entries[reference.toLowerCase()];
      if (!sourceEntry) {
        next.push(glyph);
        continue;
      }
      const range = parseReferenceRange(getValue(glyph.section, "index"));
      const total = glyphCount(sourceEntry);
      if (!range || range[0] <= 0 || range[1] > total) {
        diagnostics.warning("GLYPH_REFERENCE_INVALID", "Nexo reference glyph target or index range is invalid", { source: glyph.source, item: glyph.id, field: "index", lossy: true });
        progress = true;
        continue;
      }
      const [first, last] = range;
      const flattened = sourceEntry.chars.flatMap((row) => Array.from(row));
      const chars = [flattened.slice(first - 1, last).join("")];
      const permission = configuredPermission(glyph.section, glyph.id, sourceEntry.permission);
      const entry: GlyphEntry = {
        sourceId: glyph.id,
        targetId: sourceEntry.targetId,
        font: sourceEntry.font,
        texture: sourceEntry.texture,
        chars,
        columns: sourceEntry.columns,
        startIndex: sourceEntry.startIndex + first - 1,
        permission,
      };
      entryAliases(entries, glyph.id, entry);
      emitAuxiliaryDiagnostics(glyph, permission, diagnostics);
      progress = true;
    }
    if (next.length === 0) break;
    if (!progress) {
      for (const glyph of next) diagnostics.warning("GLYPH_REFERENCE_INVALID", "Nexo reference glyph target does not exist or forms a reference cycle", { source: glyph.source, item: glyph.id, field: "reference", lossy: true });
      break;
    }
    pending = next;
  }
  return { images, entries, sourceFiles };
}

function imageTag(entry: GlyphEntry, logicalIndex: number, colorable: boolean): string {
  const total = glyphCount(entry);
  const localIndex = logicalIndex >= 1 && logicalIndex <= total ? logicalIndex - 1 : 0;
  const [row, column] = coordinate(entry.startIndex + localIndex, entry.columns);
  const tag = "<image:" + entry.targetId + ":" + row + ":" + column + ">";
  return colorable ? tag : "<white>" + tag + "</white>";
}

function fullImageTags(entry: GlyphEntry, colorable: boolean): string {
  let logicalIndex = 1;
  const rows: string[] = [];
  for (const rowText of entry.chars) {
    const row: string[] = [];
    for (const _character of Array.from(rowText)) row.push(imageTag(entry, logicalIndex++, colorable));
    rows.push(row.join("<shift:-1>"));
  }
  return rows.join("\n");
}

function indexedImageTags(entry: GlyphEntry, start: number, end: number, colorable: boolean): string {
  // Nexo coerces a descending range to one value and falls back to the first
  // bitmap char for every out-of-range index.
  const final = Math.max(start, end);
  const count = final - start + 1;
  if (count > 10_000) return imageTag(entry, start, colorable);
  const tags: string[] = [];
  for (let index = start; index <= final; index++) tags.push(imageTag(entry, index, colorable) + (count > 1 ? "<shift:-1>" : ""));
  return tags.join("");
}

export function rewriteGlyphTags(value: JsonValue, glyphs: Record<string, GlyphEntry>, diagnostics: DiagnosticBag, source: string, item: string): JsonValue {
  if (typeof value === "string") {
    return value.replace(/(?<!\\)<(?:glyph|g):([^:>]+)((?::[^>]+)*)>/gi, (whole, rawId: string, rawArguments: string) => {
      const entry = glyphs[rawId] ?? glyphs[rawId.toLowerCase()];
      if (!entry) {
        diagnostics.warning("GLYPH_TAG_UNKNOWN", "Nexo glyph tag references an unknown or unsupported glyph: " + rawId, { source, item, field: whole, lossy: true });
        return whole;
      }
      const argumentsList = rawArguments.split(":").filter(Boolean);
      const colorable = argumentsList.some((argument) => argument === "c" || argument === "colorable");
      if (argumentsList.some((argument) => argument === "s" || argument === "shadow")) diagnostics.warning("GLYPH_TAG_SHADOW_MANUAL", "Per-use Nexo glyph shadow arguments were omitted", { source, item, field: whole, lossy: true });
      const rangeText = argumentsList.find((argument) => /^-?\d+(?:\.\.-?\d+)?$/.test(argument));
      if (!rangeText) return fullImageTags(entry, colorable);
      const range = parseReferenceRange(rangeText)!;
      if (range[1] - range[0] + 1 > 10_000) {
        diagnostics.warning("GLYPH_TAG_RANGE_TOO_LARGE", "Nexo glyph index range is too large to expand safely; only its first index was converted", { source, item, field: whole, lossy: true });
      }
      return indexedImageTags(entry, range[0], range[1], colorable);
    });
  }
  if (Array.isArray(value)) return value.map((entry) => rewriteGlyphTags(entry, glyphs, diagnostics, source, item));
  if (!isObject(value)) return value;
  return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, rewriteGlyphTags(entry, glyphs, diagnostics, source, item)]));
}
