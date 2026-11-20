// This TypeScript file is executed with Bun.

/** One provider request, as the provider reported it. */
export interface Row {
  /** Epoch milliseconds. */
  readonly at: number;
  readonly cached: number;
  readonly model: string;
  /** What changed in front of the request when it was a break, so the cause stays visible. */
  readonly part: string | null;
  /** The whole prompt: input + cacheRead + cacheWrite. */
  readonly prompt: number;
}

export interface Ledger {
  readonly rows: readonly Row[];
}

export const EMPTY_LEDGER: Ledger = { rows: [] };
export const ROW_LIMIT = 200;
/** Below this prompt size a miss costs too little to be worth a line in the chat. */
export const MISS_PROMPT = 8000;

export function record(ledger: Ledger, row: Row): Ledger {
  const rows = [...ledger.rows, row];
  return { rows: rows.length > ROW_LIMIT ? rows.slice(rows.length - ROW_LIMIT) : rows };
}

export function totals(ledger: Ledger): { readonly cached: number; readonly prompt: number } {
  let cached = 0;
  let prompt = 0;
  for (const row of ledger.rows) {
    cached += row.cached;
    prompt += row.prompt;
  }
  return { cached, prompt };
}

/** Share of the prompts this session that the provider already had cached, in percent. */
export function hitPercent(ledger: Ledger): number {
  const { cached, prompt } = totals(ledger);
  return prompt === 0 ? 0 : Math.round((100 * cached) / prompt);
}

export function statusText(ledger: Ledger): string {
  return `cache ${String(hitPercent(ledger))}%`;
}

/** A large prompt the provider mostly recomputed: the prefix in front of it had changed. */
export function isMiss(row: Row): boolean {
  return row.prompt >= MISS_PROMPT && row.cached * 2 < row.prompt;
}

function isRow(value: unknown): value is Row {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const { at, cached, model, part, prompt } = value as Partial<Row>;
  return (
    typeof at === "number" &&
    typeof cached === "number" &&
    typeof model === "string" &&
    typeof prompt === "number" &&
    (part === null || typeof part === "string")
  );
}

export function parseLedger(text: string): Ledger {
  try {
    const parsed: unknown = JSON.parse(text);
    if (typeof parsed !== "object" || parsed === null) {
      return EMPTY_LEDGER;
    }
    const { rows } = parsed as { rows?: unknown };
    return Array.isArray(rows)
      ? { rows: rows.filter((value: unknown) => isRow(value)) }
      : EMPTY_LEDGER;
  } catch {
    return EMPTY_LEDGER;
  }
}

function thousands(value: number): string {
  return Math.round(value).toLocaleString("en-US");
}

export function renderLedger(ledger: Ledger): string[] {
  const { cached, prompt } = totals(ledger);
  if (ledger.rows.length === 0) {
    return ["No request recorded yet."];
  }
  const lines = [
    `${String(hitPercent(ledger))}% cached · ${String(ledger.rows.length)} requests · ${thousands(prompt)} prompt, ${thousands(cached)} cached`,
  ];
  const misses = ledger.rows.filter(isMiss).slice(-5);
  if (misses.length === 0) {
    lines.push("No large prompt was recomputed.");
    return lines;
  }
  lines.push("Large prompts the provider recomputed:");
  for (const row of misses) {
    const part = row.part === null ? "" : ` after the ${row.part} changed`;
    lines.push(
      `  ${new Date(row.at).toISOString().slice(11, 19)} ${thousands(row.prompt)} prompt${part}`,
    );
  }
  return lines;
}
