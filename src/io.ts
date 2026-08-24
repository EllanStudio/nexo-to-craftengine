import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { parseDocument, stringify } from "yaml";
import type { DiagnosticBag } from "./diagnostics.js";
import type { JsonValue } from "./types.js";

export async function loadYaml(file: string, diagnostics: DiagnosticBag): Promise<JsonValue | undefined> {
  let text: string;
  try {
    text = await readFile(file, "utf8");
    if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  } catch (error) {
    diagnostics.error("YAML_READ_FAILED", String(error), { source: file });
    return undefined;
  }
  const document = parseDocument(text, {
    schema: "yaml-1.1",
    uniqueKeys: true,
    merge: true,
    prettyErrors: true,
  });
  if (document.errors.length > 0) {
    for (const error of document.errors) diagnostics.error("YAML_INVALID", error.message, { source: file });
    return undefined;
  }
  try {
    return document.toJS({ maxAliasCount: 100 }) as JsonValue;
  } catch (error) {
    diagnostics.error("YAML_CONVERSION_FAILED", String(error), { source: file });
    return undefined;
  }
}

export async function writeYaml(file: string, value: JsonValue): Promise<void> {
  await mkdir(dirname(file), { recursive: true });
  const text = stringify(value, { indent: 2, lineWidth: 0, defaultStringType: "PLAIN", defaultKeyType: "PLAIN" });
  await writeFile(file, text, "utf8");
}

export async function loadJson(file: string, diagnostics: DiagnosticBag): Promise<JsonValue | undefined> {
  try {
    const text = (await readFile(file, "utf8")).replace(/^\uFEFF/, "");
    return JSON.parse(text) as JsonValue;
  } catch (error) {
    diagnostics.error("JSON_INVALID", String(error), { source: file });
    return undefined;
  }
}

export async function writeJson(file: string, value: unknown): Promise<void> {
  await mkdir(dirname(file), { recursive: true });
  await writeFile(file, JSON.stringify(value, null, 2) + "\n", "utf8");
}
