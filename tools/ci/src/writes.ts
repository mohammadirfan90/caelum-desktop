// Write-exception guard (CLAUDE.md rule 5, R-SEC-04, R-SEC-07, SR-011, CR-002 D1).
//
// caelum-vehicle has no write API: any write-capable call there fails, with no exceptions.
// caelum-devices may write to a device only at a call site marked
//   // WRITE-EXCEPTION: WX-NNN
// (same line or the line above) whose entry in docs/write-exceptions.md names this crate, an
// allowed category (a, b or c) and a reviewer. Register entries with no marker in code fail too.
//
// This is a tripwire over source text, not a type-level proof: it catches the write APIs of the
// transports these crates use (std::io / tokio I/O, btleplug, bluer, hidapi, sockets). Review of
// every register entry remains required.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

export interface WriteRule {
  readonly id: string;
  readonly pattern: RegExp;
  readonly what: string;
}

/** Write-capable calls in either guarded crate. */
export const WRITE_RULES: readonly WriteRule[] = [
  { id: "io-write", pattern: /\.write\(\s*[^)\s]/, what: "I/O or characteristic write call" },
  { id: "io-write-all", pattern: /\.write_all\(/, what: "I/O write_all call" },
  { id: "io-write-vectored", pattern: /\.write_vectored\(/, what: "I/O vectored write" },
  { id: "io-write-trait", pattern: /\b(?:io::Write|AsyncWrite(?:Ext)?)\b/, what: "I/O write trait" },
  { id: "ble-write-type", pattern: /\bWriteType::/, what: "BLE characteristic write (btleplug)" },
  { id: "ble-write-descriptor", pattern: /\.write_descriptor\(/, what: "BLE descriptor write" },
  { id: "ble-write-ext", pattern: /\.write_(?:ext|io)\(/, what: "BLE write (bluer)" },
  { id: "ble-subscribe", pattern: /\.(?:un)?subscribe\(\s*[^)\s]/, what: "BLE CCCD write (subscribe)" },
  { id: "hid-report", pattern: /\.send_(?:feature|output)_report\(/, what: "HID report write" },
  { id: "socket-send-to", pattern: /\.send_to\(/, what: "socket send" },
];

/** Extra rules for caelum-vehicle, which receives only. */
export const VEHICLE_ONLY_RULES: readonly WriteRule[] = [
  { id: "vehicle-send", pattern: /\.send\(/, what: "send call (caelum-vehicle receives only)" },
  { id: "vehicle-connect", pattern: /\.connect\(/, what: "socket connect (caelum-vehicle receives only)" },
];

export const GUARDED_CRATES = ["caelum-devices", "caelum-vehicle"] as const;
export type GuardedCrate = (typeof GUARDED_CRATES)[number];

const MARKER = /WRITE-EXCEPTION:\s*(WX-\d{3})/;
const CATEGORIES = new Set(["a", "b", "c"]);

export interface RegisterEntry {
  readonly id: string;
  readonly crate: string;
  readonly category: string;
  readonly reviewer: string;
}

/** Parses the table under "## Register" in docs/write-exceptions.md. */
export function parseRegister(markdown: string): RegisterEntry[] {
  const section = markdown.split(/^## Register\s*$/m)[1] ?? "";
  return section
    .split(/\r?\n/)
    .filter((line) => /^\|\s*WX-/.test(line))
    .map((line) => {
      const cells = line.split("|").slice(1, -1).map((c) => c.trim());
      return {
        id: cells[0] ?? "",
        crate: cells[1] ?? "",
        category: (cells[3] ?? "").toLowerCase(),
        reviewer: cells[6] ?? "",
      };
    });
}

export interface SourceFile {
  readonly crate: GuardedCrate;
  readonly path: string;
  readonly text: string;
}

export interface Finding {
  readonly where: string;
  readonly message: string;
}

/** Removes a `//` comment from a line (string literals containing `//` are rare in this code). */
function code(line: string): string {
  const at = line.indexOf("//");
  return at < 0 ? line : line.slice(0, at);
}

/** Checks source files against the register; returns every violation. */
export function checkWrites(files: readonly SourceFile[], register: readonly RegisterEntry[]): Finding[] {
  const findings: Finding[] = [];
  const byId = new Map(register.map((e) => [e.id, e]));
  const usedIds = new Set<string>();

  for (const entry of register) {
    const where = `docs/write-exceptions.md ${entry.id}`;
    if (entry.crate === "caelum-vehicle") {
      findings.push({ where, message: "caelum-vehicle has no write API; it can have no exceptions" });
    } else if (entry.crate !== "caelum-devices") {
      findings.push({ where, message: `unknown crate "${entry.crate}"` });
    }
    if (!CATEGORIES.has(entry.category)) {
      findings.push({ where, message: `category must be a, b or c (got "${entry.category}")` });
    }
    if (entry.reviewer.length === 0) {
      findings.push({ where, message: "no reviewer recorded; the entry is not valid yet" });
    }
  }

  for (const file of files) {
    const lines = file.text.split(/\r?\n/);
    const rules = file.crate === "caelum-vehicle" ? [...WRITE_RULES, ...VEHICLE_ONLY_RULES] : WRITE_RULES;
    lines.forEach((line, index) => {
      const marker = MARKER.exec(line) ?? MARKER.exec(lines[index - 1] ?? "");
      if (marker?.[1] !== undefined) usedIds.add(marker[1]);
      const where = `${file.path}:${String(index + 1)}`;
      for (const rule of rules) {
        if (!rule.pattern.test(code(line))) continue;
        const label = `${rule.what} [${rule.id}]`;
        if (file.crate === "caelum-vehicle") {
          findings.push({ where, message: `${label}: caelum-vehicle has no write API (SR-011)` });
          continue;
        }
        const id = marker?.[1];
        if (id === undefined) {
          findings.push({ where, message: `${label} without a WRITE-EXCEPTION marker` });
          continue;
        }
        const entry = byId.get(id);
        if (entry === undefined) {
          findings.push({ where, message: `${label}: ${id} is not in docs/write-exceptions.md` });
        } else if (entry.crate !== file.crate) {
          findings.push({ where, message: `${label}: ${id} is registered for ${entry.crate}` });
        }
      }
    });
  }

  for (const entry of register) {
    if (!usedIds.has(entry.id)) {
      findings.push({
        where: `docs/write-exceptions.md ${entry.id}`,
        message: "no call site carries this marker; remove the stale entry",
      });
    }
  }
  return findings;
}

function rustFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return rustFiles(path);
    return path.endsWith(".rs") ? [path] : [];
  });
}

/** Reads the guarded crates' `src/` trees from the repository. */
export function readSources(repoRoot: string): SourceFile[] {
  return GUARDED_CRATES.flatMap((crate) =>
    rustFiles(join(repoRoot, "crates", crate, "src")).map((path) => ({
      crate,
      path: relative(repoRoot, path).replaceAll("\\", "/"),
      text: readFileSync(path, "utf8"),
    })),
  );
}

export function readRegister(repoRoot: string): RegisterEntry[] {
  return parseRegister(readFileSync(join(repoRoot, "docs", "write-exceptions.md"), "utf8"));
}
