import { access, copyFile, mkdir, readdir, readFile, stat, writeFile } from "node:fs/promises";
import { basename, dirname, join, relative } from "node:path";
import type { DiagnosticBag } from "./diagnostics.js";
import type { JsonObject } from "./types.js";

async function exists(path: string): Promise<boolean> {
  try { await access(path); return true; } catch { return false; }
}

export async function findResourcePackRoot(input: string): Promise<string | undefined> {
  const candidates = [join(input, "pack"), join(input, "resourcepack"), input];
  for (const candidate of candidates) if (await exists(join(candidate, "assets"))) return candidate;
  return undefined;
}

async function sameFile(left: string, right: string): Promise<boolean> {
  try {
    const [a, b] = await Promise.all([readFile(left), readFile(right)]);
    return a.equals(b);
  } catch { return false; }
}

export async function copyResourcePack(sourceRoot: string, outputRoot: string, diagnostics: DiagnosticBag, blueprintRoot?: string): Promise<number> {
  let copied = 0;
  const visit = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      const source = join(directory, entry.name);
      const relativePath = relative(sourceRoot, source);
      if (relativePath.toLowerCase() === "pack.mcmeta") {
        diagnostics.info("PACK_MCMETA_SKIPPED", "CraftEngine generates versioned pack.mcmeta; the Nexo file was not copied", { source });
        continue;
      }
      const target = join(outputRoot, relativePath);
      if (entry.isSymbolicLink()) {
        diagnostics.warning("RESOURCE_SYMLINK_SKIPPED", "Resource-pack symbolic link was skipped", { source, lossy: true });
        continue;
      }
      if (entry.isDirectory()) {
        await visit(source);
      } else if (entry.isFile()) {
        if (/[A-Z]/.test(relativePath.replaceAll("\\", "/"))) diagnostics.warning("RESOURCE_PATH_UPPERCASE", "Minecraft resource paths should be lowercase: " + relativePath, { source, lossy: true });
        let destination = target;
        if (blueprintRoot && entry.name.toLowerCase().endsWith(".bbmodel")) {
          const parts = relativePath.replaceAll("\\", "/").split("/");
          const assets = parts.lastIndexOf("assets");
          if (assets < 0 || parts.length < assets + 4) {
            diagnostics.error("BBMODEL_ASSET_PATH_INVALID", "Nexo bbmodel must be below assets/<namespace>/<category>/", { source, lossy: true });
            continue;
          }
          const namespace = parts[assets + 1]!;
          destination = join(blueprintRoot, namespace, ...parts.slice(assets + 3));
        }
        await mkdir(dirname(destination), { recursive: true });
        if (await exists(destination)) {
          if (!(await sameFile(source, destination))) diagnostics.error("RESOURCE_COPY_CONFLICT", "Different resources map to the same output path", { source, field: destination });
          continue;
        }
        await copyFile(source, destination);
        copied++;
      }
    }
  };
  await visit(sourceRoot);
  return copied;
}

export async function writeLanguageResources(root: JsonObject, outputResourcePack: string, diagnostics: DiagnosticBag, source: string): Promise<number> {
  const global = root.global && typeof root.global === "object" && !Array.isArray(root.global) ? root.global as JsonObject : {};
  let count = 0;
  for (const [locale, value] of Object.entries(root)) {
    if (locale.toLowerCase() === "global" || typeof value !== "object" || value === null || Array.isArray(value)) continue;
    const merged = { ...global, ...value };
    const file = join(outputResourcePack, "assets", "nexo", "lang", locale.toLowerCase().replaceAll("-", "_") + ".json");
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, JSON.stringify(merged, null, 2) + "\n", "utf8");
    count++;
  }
  if (Object.keys(global).length > 0 && count === 0) diagnostics.warning("GLOBAL_LANGUAGE_SCOPE", "Nexo global translations need an explicit locale set; no locale file could be generated", { source, field: "global", lossy: true });
  return count;
}
