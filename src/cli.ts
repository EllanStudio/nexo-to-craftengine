#!/usr/bin/env node
import { resolve } from "node:path";
import { convert, type ConvertOptions } from "./converter.js";

const HELP = [
  "Nexo 1.26 -> CraftEngine 26.8 semantic converter (TypeScript)",
  "",
  "Usage:",
  "  nexo2ce <Nexo目录> <CE输出目录> [options]",
  "  node dist/src/cli.js <Nexo目录> <CE输出目录> [options]",
  "",
  "Options:",
  "  --namespace <id>                Explicitly rename IDs (default: auto-detect Nexo source namespace)",
  "  --client-mode <mode>            modern | hybrid | legacy (default: hybrid)",
  "  --cmd-policy <policy>           preserve | allocate | omit (default: preserve)",
  "  --strict                        Fail if any conversion is diagnosed as lossy",
  "  --force                         Replace a non-empty output directory",
  "  --no-audit                      Skip model/texture resource graph audit",
  "  -h, --help                      Show this help",
  "  -v, --version                   Show version",
  "",
  "Important:",
  "  Unqualified Minecraft resource locations keep the vanilla default namespace 'minecraft'.",
  "  Use --cmd-policy allocate only when all Nexo item configs are present; allocation is material-scoped.",
].join("\n");

export function parseArguments(args: string[]): ConvertOptions | "help" | "version" {
  const positional: string[] = [];
  const values = new Map<string, string>();
  let strict = false;
  let force = false;
  let audit = true;
  for (let index = 0; index < args.length; index++) {
    const argument = args[index]!;
    if (argument === "-h" || argument === "--help") return "help";
    if (argument === "-v" || argument === "--version") return "version";
    if (argument === "--strict") { strict = true; continue; }
    if (argument === "--force") { force = true; continue; }
    if (argument === "--no-audit") { audit = false; continue; }
    if (["--namespace", "--client-mode", "--cmd-policy"].includes(argument)) {
      const next = args[++index];
      if (!next) throw new Error("Missing value after " + argument);
      values.set(argument, next);
      continue;
    }
    if (argument.startsWith("-")) throw new Error("Unknown option: " + argument);
    if (argument === "convert" && positional.length === 0) continue;
    positional.push(argument);
  }
  if (positional.length !== 2) throw new Error("Expected Nexo input and CraftEngine output directories\n\n" + HELP);
  const clientMode = values.get("--client-mode") ?? "hybrid";
  const cmdPolicy = values.get("--cmd-policy") ?? "preserve";
  if (!new Set(["modern", "hybrid", "legacy"]).has(clientMode)) throw new Error("Invalid --client-mode: " + clientMode);
  if (!new Set(["preserve", "allocate", "omit"]).has(cmdPolicy)) throw new Error("Invalid --cmd-policy: " + cmdPolicy);
  return {
    input: resolve(positional[0]!),
    output: resolve(positional[1]!),
    namespace: values.get("--namespace"),
    clientMode: clientMode as ConvertOptions["clientMode"],
    cmdPolicy: cmdPolicy as ConvertOptions["cmdPolicy"],
    strict,
    force,
    audit,
  };
}

async function main(): Promise<void> {
  const parsed = parseArguments(process.argv.slice(2));
  if (parsed === "help") {
    console.log(HELP);
    return;
  }
  if (parsed === "version") {
    console.log("0.1.0");
    return;
  }
  const result = await convert(parsed);
  const counts = result.diagnostics.counts();
  console.log("Converted " + result.itemCount + " items, " + result.furnitureCount + " furniture, " + result.blockCount + " blocks, " + result.recipeCount + " recipes, " + result.soundCount + " sounds, " + result.glyphCount + " glyph images.");
  console.log("Copied " + result.resourceCount + " resource files. Diagnostics: " + counts.error + " errors, " + counts.warning + " warnings, " + counts.lossy + " lossy.");
  for (const line of result.diagnostics.formatLines().slice(0, 100)) console.error(line);
  if (result.diagnostics.items.length > 100) console.error("... " + (result.diagnostics.items.length - 100) + " more diagnostics are in the JSON report.");
  if (result.reportFile) console.log("Report: " + result.reportFile);
  if (!result.success) process.exitCode = 2;
}

main().catch((error: unknown) => {
  console.error("nexo2ce: " + (error instanceof Error ? error.message : String(error)));
  process.exitCode = 1;
});
