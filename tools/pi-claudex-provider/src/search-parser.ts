import type { WebSearchResult } from "./protocol.ts";

const FIELD_RE = /^\s*(Title|URL|Snippet):\s*(.+)$/mu;

interface PendingTriplet {
  title: string;
  url?: string;
  snippet: string;
}

function flushPending(pending: PendingTriplet | null): WebSearchResult | null {
  if (pending?.url !== undefined) {
    return { title: pending.title, url: pending.url, snippet: pending.snippet };
  }
  return null;
}

function processField(
  pending: PendingTriplet | null,
  results: WebSearchResult[],
  field: string,
  value: string,
): PendingTriplet | null {
  if (field === "Title") {
    const flushed = flushPending(pending);
    if (flushed !== null) {
      results.push(flushed);
    }
    return { title: value, snippet: "" };
  }
  if (pending === null) {
    return null;
  }
  if (field === "URL") {
    return { ...pending, url: value };
  }
  // Field === "Snippet" (guaranteed by FIELD_RE)
  return { ...pending, snippet: value };
}

function extractTriplets(text: string): WebSearchResult[] {
  const results: WebSearchResult[] = [];
  let pending: PendingTriplet | null = null;
  for (const line of text.split("\n")) {
    const hit = FIELD_RE.exec(line);
    if (hit !== null) {
      const field = hit[1] ?? "";
      const value = (hit[2] ?? "").trim();
      pending = processField(pending, results, field, value);
    }
  }
  const final = flushPending(pending);
  if (final !== null) {
    results.push(final);
  }
  return results;
}

function looseExtract(text: string): WebSearchResult[] {
  const results: WebSearchResult[] = [];
  const looseRe = /^Title:\s*(.+)\nURL:\s*(.+)\nSnippet:\s*(.+)$/gmu;
  for (const match of text.matchAll(looseRe)) {
    results.push({
      title: match[1]?.trim() ?? "",
      url: match[2]?.trim() ?? "",
      snippet: match[3]?.trim() ?? "",
    });
  }
  return results;
}

export function parseSearchTriplets(text: string): WebSearchResult[] {
  const structured = extractTriplets(text);
  if (structured.length > 0) {
    return structured;
  }
  return looseExtract(text);
}
