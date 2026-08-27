import type { JsonObject } from "./types.js";

export type Severity = "info" | "warning" | "error";

export interface Diagnostic {
  severity: Severity;
  code: string;
  message: string;
  source?: string;
  item?: string;
  field?: string;
  lossy?: boolean;
  context?: JsonObject;
}

export class DiagnosticBag {
  readonly items: Diagnostic[] = [];
  private readonly seen = new Set<string>();

  add(diagnostic: Diagnostic): void {
    const normalized: Diagnostic = { ...diagnostic, lossy: diagnostic.lossy ?? false };
    const signature = JSON.stringify(normalized);
    if (this.seen.has(signature)) return;
    this.seen.add(signature);
    this.items.push(normalized);
  }

  info(code: string, message: string, details: Omit<Diagnostic, "severity" | "code" | "message"> = {}): void {
    this.add({ severity: "info", code, message, ...details });
  }

  warning(code: string, message: string, details: Omit<Diagnostic, "severity" | "code" | "message"> = {}): void {
    this.add({ severity: "warning", code, message, ...details });
  }

  error(code: string, message: string, details: Omit<Diagnostic, "severity" | "code" | "message"> = {}): void {
    this.add({ severity: "error", code, message, ...details });
  }

  hasErrors(): boolean {
    return this.items.some((item) => item.severity === "error");
  }

  hasLossy(): boolean {
    return this.items.some((item) => item.lossy === true);
  }

  counts(): Record<string, number> {
    const counts: Record<string, number> = { info: 0, warning: 0, error: 0, lossy: 0 };
    for (const item of this.items) {
      counts[item.severity] = (counts[item.severity] ?? 0) + 1;
      if (item.lossy) counts.lossy = (counts.lossy ?? 0) + 1;
    }
    return counts;
  }

  formatLines(): string[] {
    return this.items.map((item) => {
      const where = [item.source, item.item, item.field].filter(Boolean).join("/");
      const suffix = where ? " (" + where + ")" : "";
      return "[" + item.severity.toUpperCase() + (item.lossy ? " LOSSY" : "") + "] " + item.code + ": " + item.message + suffix;
    });
  }
}
