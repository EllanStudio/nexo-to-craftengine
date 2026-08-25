import { basename, extname, isAbsolute, join, relative } from "node:path";
import type { DiagnosticBag } from "./diagnostics.js";
import { getBoolean, getNumber, getObject, getString, type JsonObject } from "./types.js";

export interface CategoryItem {
  source: string;
  sourceId: string;
  targetId: string;
  config: JsonObject;
}

export interface CategoryConversionOptions {
  root: string;
  namespace: string;
  items: CategoryItem[];
  inventory?: JsonObject;
  inventorySource?: string;
  rewriteText?: (text: string, field: string) => string;
  diagnostics: DiagnosticBag;
}

interface CategoryMetadata {
  name: string;
  icon: string;
  slot?: number;
}

interface FileGroup {
  relativeFile: string;
  relativeStem: string;
  items: CategoryItem[];
}

interface CategoryNode {
  kind: "root" | "directory" | "file";
  key: string;
  label: string;
  group?: FileGroup;
  children: CategoryNode[];
  id?: string;
}

function inside(parent: string, child: string): string | undefined {
  const candidate = relative(parent, child);
  if (candidate === "" || isAbsolute(candidate) || candidate === ".." || candidate.startsWith("../") || candidate.startsWith("..\\")) return undefined;
  return candidate.replaceAll("\\", "/");
}

function relativeItemFile(root: string, source: string): string {
  for (const directory of [join(root, "items"), join(root, "item")]) {
    const candidate = inside(directory, source);
    if (candidate !== undefined) return candidate;
  }
  return basename(source);
}

function withoutYamlExtension(path: string): string {
  const extension = extname(path);
  return extension.toLowerCase() === ".yml" || extension.toLowerCase() === ".yaml"
    ? path.slice(0, -extension.length)
    : path;
}

function categorySlug(path: string): string {
  const segments = path.replaceAll("\\", "/").split("/").filter(Boolean).map((segment) => {
    const normalized = segment.normalize("NFKD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
    return normalized.replace(/[^a-z0-9._-]+/g, "_").replace(/^[_./-]+|[_./-]+$/g, "") || "category";
  });
  return segments.join("/") || "category";
}

function humanize(value: string): string {
  return value.split(/[_-]+/).filter(Boolean).map((part) => part.length === 0 ? part : part[0]!.toUpperCase() + part.slice(1)).join(" ") || value;
}

function nestedObject(root: JsonObject | undefined, dottedPath: string): JsonObject | undefined {
  if (!root) return undefined;
  let current: JsonObject | undefined = root;
  for (const part of dottedPath.split(".").filter(Boolean)) {
    current = current ? getObject(current, part) : undefined;
    if (!current) return undefined;
  }
  return current;
}

function inventorySection(inventory: JsonObject | undefined): JsonObject {
  if (!inventory) return {};
  return getObject(inventory, "NexoInventory") ?? getObject(inventory, "nexo_inventory") ?? inventory;
}

function inventoryLayout(section: JsonObject): JsonObject | undefined {
  return getObject(section, "layout") ?? getObject(section, "menu_layout");
}

function layoutPath(relativePath: string, mode: "FILE" | "DIRECTORY"): string {
  if (mode === "DIRECTORY") return withoutYamlExtension(relativePath).replaceAll("/", ".");
  return withoutYamlExtension(basename(relativePath));
}

function firstTarget(node: CategoryNode): string | undefined {
  if (node.group?.items[0]) return node.group.items[0].targetId;
  for (const child of node.children) {
    const target = firstTarget(child);
    if (target) return target;
  }
  return undefined;
}

function resolveIcon(
  rawIcon: string | undefined,
  fallback: string,
  namespace: string,
  sourceTargets: Map<string, string>,
  targets: Set<string>,
  diagnostics: DiagnosticBag,
  source: string | undefined,
  field: string,
): string {
  const icon = rawIcon?.trim();
  if (!icon) return fallback;
  const separator = icon.indexOf(":");
  const sourceId = separator < 0 ? icon : icon.slice(separator + 1);
  const sourceNamespace = separator < 0 ? "nexo" : icon.slice(0, separator).toLowerCase();
  const mapped = sourceTargets.get(sourceId);
  if (mapped) return mapped;
  if (targets.has(icon)) return icon;
  if (separator >= 0 && sourceNamespace !== "nexo") return icon;
  const targetCandidate = namespace + ":" + sourceId;
  if (targets.has(targetCandidate)) return targetCandidate;
  diagnostics.warning("CATEGORY_ICON_FALLBACK", "Category icon " + icon + " does not identify a converted item; used " + fallback, {
    source,
    field,
    lossy: true,
  });
  return fallback;
}

function categoryMetadata(
  node: CategoryNode,
  mode: "FILE" | "DIRECTORY",
  section: JsonObject,
  sourceTargets: Map<string, string>,
  targets: Set<string>,
  namespace: string,
  diagnostics: DiagnosticBag,
  source: string | undefined,
  rewriteText: ((text: string, field: string) => string) | undefined,
): CategoryMetadata {
  const layout = inventoryLayout(section);
  const relativePath = node.kind === "file" ? node.group!.relativeFile : node.key;
  const path = layoutPath(relativePath, mode);
  const configured = nestedObject(layout, path);
  const styledNames = getBoolean(section, "style_default_names", true);
  const defaultLabel = styledNames ? humanize(node.label) : node.kind === "file" ? basename(node.group!.relativeFile) : node.label;
  const configuredName = configured
    ? getString(configured, "itemname") ?? getString(configured, "displayname") ?? getString(configured, "title")
    : undefined;
  const renderedName = configuredName && rewriteText
    ? rewriteText(configuredName, "NexoInventory.layout." + path + ".name")
    : configuredName;
  const fallback = firstTarget(node) ?? "minecraft:stone";
  const configuredIcon = configured ? getString(configured, "icon") : undefined;
  const icon = resolveIcon(
    configuredIcon ?? (node.kind === "directory" ? getString(section, "directory_icon") : undefined),
    fallback,
    namespace,
    sourceTargets,
    targets,
    diagnostics,
    source,
    "NexoInventory.layout." + path + ".icon",
  );
  const rawSlot = configured ? getNumber(configured, "slot") : undefined;
  const slot = rawSlot !== undefined && Number.isInteger(rawSlot) && rawSlot > 0 ? rawSlot - 1 : undefined;
  return {
    name: "<!i><green>" + (renderedName ?? defaultLabel) + "</green>",
    icon,
    slot,
  };
}

function assignPriorities<T extends { metadata: CategoryMetadata; key: string }>(
  entries: T[],
  diagnostics: DiagnosticBag,
  source: string | undefined,
): T[] {
  const used = new Set<number>();
  const pending: T[] = [];
  for (const entry of entries) {
    const requested = entry.metadata.slot;
    if (requested === undefined || used.has(requested)) {
      if (requested !== undefined) diagnostics.warning("CATEGORY_SLOT_CONFLICT", "Multiple Nexo inventory entries request slot " + (requested + 1) + "; assigned the next free position", {
        source,
        field: "NexoInventory.layout",
        lossy: true,
      });
      pending.push(entry);
      continue;
    }
    used.add(requested);
  }
  let cursor = 0;
  for (const entry of pending) {
    while (used.has(cursor)) cursor++;
    entry.metadata.slot = cursor;
    used.add(cursor++);
  }
  return entries.sort((a, b) => (a.metadata.slot! - b.metadata.slot!) || a.key.localeCompare(b.key, "en"));
}

function allocateIds(nodes: CategoryNode[], namespace: string): void {
  const used = new Set<string>();
  for (const node of [...nodes].sort((a, b) => a.key.localeCompare(b.key, "en"))) {
    const base = categorySlug(node.key);
    let path = base;
    let suffix = 2;
    while (used.has(path)) path = base + "-" + suffix++;
    used.add(path);
    node.id = namespace + ":" + path;
  }
}

function convertedGroups(options: CategoryConversionOptions): FileGroup[] {
  const byFile = new Map<string, FileGroup>();
  for (const item of options.items) {
    if (getBoolean(item.config, "excludeFromInventory", false)) continue;
    const relativeFile = relativeItemFile(options.root, item.source);
    const group = byFile.get(relativeFile) ?? {
      relativeFile,
      relativeStem: withoutYamlExtension(relativeFile),
      items: [],
    };
    group.items.push(item);
    byFile.set(relativeFile, group);
  }
  return [...byFile.values()].sort((a, b) => a.relativeFile.localeCompare(b.relativeFile, "en"));
}

function convertFileCategories(options: CategoryConversionOptions, section: JsonObject, groups: FileGroup[]): JsonObject {
  const categories: JsonObject = {};
  const sourceTargets = new Map(options.items.map((item) => [item.sourceId, item.targetId]));
  const targets = new Set(options.items.map((item) => item.targetId));
  const nodes: CategoryNode[] = groups.map((group) => ({
    kind: "file",
    key: group.relativeStem,
    label: withoutYamlExtension(basename(group.relativeFile)),
    group,
    children: [],
  }));
  allocateIds(nodes, options.namespace);
  const ordered = assignPriorities(nodes.map((node) => ({
    key: node.key,
    node,
    metadata: categoryMetadata(node, "FILE", section, sourceTargets, targets, options.namespace, options.diagnostics, options.inventorySource, options.rewriteText),
  })), options.diagnostics, options.inventorySource);
  for (const entry of ordered) {
    categories[entry.node.id!] = {
      name: entry.metadata.name,
      icon: entry.metadata.icon,
      priority: entry.metadata.slot!,
      list: entry.node.group!.items.map((item) => item.targetId),
    };
  }
  return categories;
}

function buildDirectoryTree(groups: FileGroup[]): CategoryNode {
  const root: CategoryNode = { kind: "root", key: "", label: "", children: [] };
  const directories = new Map<string, CategoryNode>([["", root]]);
  for (const group of groups) {
    const parts = group.relativeStem.split("/").filter(Boolean);
    let parent = root;
    let directoryPath = "";
    for (const part of parts.slice(0, -1)) {
      directoryPath = directoryPath ? directoryPath + "/" + part : part;
      let directory = directories.get(directoryPath);
      if (!directory) {
        directory = { kind: "directory", key: directoryPath, label: part, children: [] };
        directories.set(directoryPath, directory);
        parent.children.push(directory);
      }
      parent = directory;
    }
    parent.children.push({
      kind: "file",
      key: group.relativeStem,
      label: parts.at(-1) ?? group.relativeStem,
      group,
      children: [],
    });
  }
  return root;
}

function flattenNodes(root: CategoryNode): CategoryNode[] {
  const result: CategoryNode[] = [];
  const visit = (node: CategoryNode): void => {
    for (const child of node.children) {
      result.push(child);
      visit(child);
    }
  };
  visit(root);
  return result;
}

function convertDirectoryCategories(options: CategoryConversionOptions, section: JsonObject, groups: FileGroup[]): JsonObject {
  const categories: JsonObject = {};
  const sourceTargets = new Map(options.items.map((item) => [item.sourceId, item.targetId]));
  const targets = new Set(options.items.map((item) => item.targetId));
  const root = buildDirectoryTree(groups);
  const nodes = flattenNodes(root);
  allocateIds(nodes, options.namespace);

  const metadata = new Map<CategoryNode, CategoryMetadata>();
  for (const node of nodes) metadata.set(node, categoryMetadata(node, "DIRECTORY", section, sourceTargets, targets, options.namespace, options.diagnostics, options.inventorySource, options.rewriteText));

  const orderChildren = (parent: CategoryNode): void => {
    const ordered = assignPriorities(parent.children.map((node) => ({ key: node.key, node, metadata: metadata.get(node)! })), options.diagnostics, options.inventorySource);
    parent.children = ordered.map((entry) => entry.node);
    for (const child of parent.children) orderChildren(child);
  };
  orderChildren(root);

  const topLevel = new Set(root.children);
  for (const node of nodes) {
    const entry: JsonObject = {
      name: metadata.get(node)!.name,
      icon: metadata.get(node)!.icon,
      list: node.kind === "file"
        ? node.group!.items.map((item) => item.targetId)
        : node.children.map((child) => "#" + child.id!),
    };
    if (topLevel.has(node)) entry.priority = metadata.get(node)!.slot!;
    else entry.hidden = true;
    categories[node.id!] = entry;
  }
  return categories;
}

export function convertCategories(options: CategoryConversionOptions): JsonObject {
  const groups = convertedGroups(options);
  if (groups.length === 0) return {};
  const section = inventorySection(options.inventory);
  const rawType = getString(section, "type")?.trim().toUpperCase();
  const type = rawType === "DIRECTORY" ? "DIRECTORY" : "FILE";
  if (rawType !== undefined && rawType !== "FILE" && rawType !== "DIRECTORY") {
    options.diagnostics.warning("CATEGORY_INVENTORY_TYPE_INVALID", "Unknown Nexo inventory type " + rawType + "; used FILE", {
      source: options.inventorySource,
      field: "NexoInventory.type",
      lossy: true,
    });
  }
  return type === "DIRECTORY"
    ? convertDirectoryCategories(options, section, groups)
    : convertFileCategories(options, section, groups);
}
