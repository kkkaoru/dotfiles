// Sentence splitting and token-budget packing for a text longer than the model's window.
//
// The counts come from the server's /v1/tokenize, so nothing here estimates from characters: a
// Japanese character is about one token, and a 4-tokens-per-character guess halves the chunk.

import { plamoPrompt } from "../../claude-plamo-translate/hooks/translate.ts";
import type { ResponseLike } from "./server.ts";
import { isRecord } from "./settings.ts";

const TERMINATORS = "。．！？!?";
const CLOSERS = "」』）)】〉》\"'”’";
const SPACE = /\s/u;
/** A text this short cannot fill the window: about one token per Japanese character. */
const SHORT_CHARS = 512;
/** Room for a translation that runs a little past its source in tokens. */
const ANSWER_SLACK = 16;

/** POSTs a JSON body to the server; `call` in server.ts, injected so this module stays testable. */
export type Post = (path: string, body: unknown) => Promise<ResponseLike>;

export interface Counted {
  readonly counts: number[];
  readonly window: number;
}

export interface Chunk {
  readonly text: string;
  readonly maxTokens: number;
}

export interface Plan {
  readonly chunks: readonly Chunk[];
  readonly join: string;
}

/** The count at an index: these arrays are always as long as the texts they describe. */
const countAt = (counts: readonly number[], index: number): number => counts[index] ?? 0;

const isTerminator = (text: string, index: number): boolean => {
  const char = text.slice(index, index + 1);
  if (TERMINATORS.includes(char) || char === "\n") {
    return true;
  }
  return (
    char === "." && (index + 1 === text.length || SPACE.test(text.slice(index + 1, index + 2)))
  );
};

/** The index after a terminator, its closing quotes and the spaces that follow them. */
const afterSeparators = (text: string, from: number): number => {
  let next = from;
  while (next < text.length && CLOSERS.includes(text.slice(next, next + 1))) {
    next += 1;
  }
  while (next < text.length && SPACE.test(text.slice(next, next + 1))) {
    next += 1;
  }
  return next;
};

/** Sentence-sized units that concatenate back to the input: every unit keeps its trailing space. */
export function units(text: string): string[] {
  const out: string[] = [];
  let start = 0;
  let index = 0;
  while (index < text.length) {
    if (isTerminator(text, index)) {
      const next = afterSeparators(text, index + 1);
      out.push(text.slice(start, next));
      start = next;
      index = next;
    } else {
      index += 1;
    }
  }
  if (start < text.length) {
    out.push(text.slice(start));
  }
  return out;
}

/** Groups consecutive unit indices so each group's token count stays within the budget. */
export function pack(counts: readonly number[], budget: number): number[][] {
  const groups: number[][] = [];
  let current: number[] = [];
  let total = 0;
  for (let index = 0; index < counts.length; index += 1) {
    const count = countAt(counts, index);
    if (current.length > 0 && total + count > budget) {
      groups.push(current);
      current = [];
      total = 0;
    }
    current.push(index);
    total += count;
  }
  if (current.length > 0) {
    groups.push(current);
  }
  return groups;
}

/** The counts and window from a /v1/tokenize body, or null when the shape is not what we expect. */
function counted(body: unknown, expected: number): Counted | null {
  if (!isRecord(body)) {
    return null;
  }
  const { tokens, window } = body;
  if (!Array.isArray(tokens) || typeof window !== "number") {
    return null;
  }
  const counts = tokens.filter((count): count is number => typeof count === "number");
  return counts.length === expected ? { counts, window } : null;
}

/** Exact token counts, or null where the endpoint is missing (the Python and llama.cpp fallbacks). */
export async function tokenCounts(post: Post, texts: readonly string[]): Promise<Counted | null> {
  try {
    const response = await post("/v1/tokenize", { texts });
    if (!response.ok) {
      return null;
    }
    return counted(await response.json(), texts.length);
  } catch {
    return null;
  }
}

/** A piece cut in two at the proportional point, or null when it is already one character. */
const splitAt = (piece: string, count: number, budget: number): [string, string] | null => {
  const cut = Math.min(piece.length - 1, Math.floor((piece.length * budget) / count));
  return cut < 1 ? null : [piece.slice(0, cut), piece.slice(cut)];
};

/** Cuts one unit until it fits, measuring each half; false when the server cannot count. */
async function fitPiece(
  post: Post,
  pieces: string[],
  counts: number[],
  index: number,
  budget: number,
): Promise<boolean> {
  // A proportional cut is within a few tokens; eight rounds is far past that accuracy.
  for (let round = 0; round < 8 && countAt(counts, index) > budget; round += 1) {
    const split = splitAt(pieces[index] ?? "", countAt(counts, index), budget);
    if (split === null) {
      break;
    }
    // oxlint-disable-next-line no-await-in-loop -- each cut is measured before the next one
    const measured = await tokenCounts(post, split);
    if (measured === null) {
      return false;
    }
    pieces.splice(index, 1, ...split);
    counts.splice(index, 1, countAt(measured.counts, 0), countAt(measured.counts, 1));
  }
  return true;
}

/** Cuts every unit that is over budget, until the whole text can be packed. */
async function fit(
  post: Post,
  pieces: string[],
  counts: number[],
  budget: number,
): Promise<boolean> {
  for (let index = 0; index < pieces.length; index += 1) {
    // oxlint-disable-next-line no-await-in-loop -- each cut is measured before the next one
    if (!(await fitPiece(post, pieces, counts, index, budget))) {
      return false;
    }
  }
  return true;
}

/** Chunks for a text that would overflow the window; null keeps the single-request path. */
export async function plan(
  post: Post,
  text: string,
  lang: string,
  cap: number,
): Promise<Plan | null> {
  if (text.length <= SHORT_CHARS) {
    return null;
  }
  const template = await tokenCounts(post, [plamoPrompt("", lang)]);
  if (template === null) {
    return null;
  }
  // Half the window for the source, half for the answer: both occupy the window while decoding.
  // A translation runs about as long as its source in tokens.
  const budget = Math.floor((template.window - countAt(template.counts, 0)) / 2);
  if (budget <= 0) {
    return null;
  }
  const pieces = units(text);
  const measured = await tokenCounts(post, pieces);
  if (measured === null) {
    return null;
  }
  const { counts } = measured;
  if (!(await fit(post, pieces, counts, budget))) {
    return null;
  }
  const groups = pack(counts, budget);
  if (groups.length <= 1) {
    return null;
  }
  const chunks = groups.map((group) => {
    const tokens = group.reduce((sum, index) => sum + countAt(counts, index), 0);
    return {
      text: pieces.slice(countAt(group, 0), countAt(group, group.length - 1) + 1).join(""),
      maxTokens: Math.min(cap, tokens + ANSWER_SLACK),
    };
  });
  return { chunks, join: lang === "ja" ? "" : " " };
}
