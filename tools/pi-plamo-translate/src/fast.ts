// This TypeScript file is executed with Bun.
import {
  fencedLines,
  translateMarkdown,
  translateProse,
  type Translator,
} from "../../claude-plamo-translate/hooks/translate.ts";

const MARKER = /^(\s*(?:[-*+]\s+|\d+[.)]\s+|#{1,6}\s+|>\s*)*)(.+)$/u;
const CHUNK_LINES = 2;
// A block opener (list item, heading, quote, table row, fence) is never joined to the line above.
const BLOCK_START = /^\s*(?:[-*+]\s+|\d+[.)]\s+|#{1,6}\s+|>|\||`{3,}|~{3,})/u;
// Two trailing spaces or a backslash are markdown's hard break, which stays as written.
const HARD_BREAK = /(?: {2,}|\\)$/u;

interface Piece {
  readonly body: string;
  readonly index: number;
  readonly marker: string;
}

export function textAt(values: readonly string[], index: number, fallback: string): string {
  const value = values[index];
  return typeof value === "string" ? value : fallback;
}

function pieceOf(line: string, index: number): Piece | undefined {
  const match = MARKER.exec(line);
  if (match === null) {
    return undefined;
  }
  return { body: textAt(match, 2, line), index, marker: textAt(match, 1, "") };
}

function piecesOf(lines: readonly string[], fenced: readonly boolean[]): readonly Piece[] {
  return lines.flatMap((line, index) => {
    if (fenced[index] === true) {
      return [];
    }
    const piece = pieceOf(line, index);
    return piece === undefined ? [] : [piece];
  });
}

function stitch(
  lines: readonly string[],
  pieces: readonly Piece[],
  parts: readonly string[],
): string {
  const replacements = new Map(
    pieces.map((piece, index) => [
      piece.index,
      `${piece.marker}${textAt(parts, index, piece.body)}`,
    ]),
  );
  return lines.map((line, index) => replacements.get(index) ?? line).join("\n");
}

async function translateBodies(
  bodies: readonly string[],
  translate: Translator,
): Promise<readonly string[]> {
  if (bodies.length === 0) {
    return [];
  }
  const head = bodies.slice(0, CHUNK_LINES);
  const joined = head.join("\n");
  const once = head.length === 1 ? undefined : await translateProse(joined, translate);
  const parts = once === undefined || once === joined ? undefined : once.split("\n");
  const translated =
    parts !== undefined && parts.length === head.length
      ? parts
      : await Promise.all(head.map((body) => translateProse(body, translate)));
  return [...translated, ...(await translateBodies(bodies.slice(CHUNK_LINES), translate))];
}

/** Joins the lines of a hard-wrapped paragraph: a line break inside a sentence would otherwise be
 *  translated as one, leaving the sentence split. */
function reflowed(lines: readonly string[], fenced: readonly boolean[]): string[] {
  const merged: string[] = [];
  lines.forEach((line, index) => {
    const previous = merged.at(-1);
    const joins =
      previous !== undefined &&
      line.trim() !== "" &&
      !BLOCK_START.test(line) &&
      !BLOCK_START.test(previous) &&
      !HARD_BREAK.test(previous) &&
      fenced[index] !== true &&
      fenced[index - 1] !== true;
    if (joins) {
      merged[merged.length - 1] = `${previous} ${line.trim()}`;
      return;
    }
    merged.push(line);
  });
  return merged;
}

/** Two lines per request. A reply that merges lines falls back to one request per line. */
export async function translateFast(text: string, translate: Translator): Promise<string> {
  const lines = text.split("\n");
  if (lines.some((line) => line.trimStart().startsWith("|"))) {
    return translateMarkdown(text, translate);
  }
  const merged = reflowed(lines, fencedLines(lines));
  const pieces = piecesOf(merged, fencedLines(merged));
  if (pieces.length === 0) {
    return text;
  }
  return stitch(
    merged,
    pieces,
    await translateBodies(
      pieces.map((piece) => piece.body),
      translate,
    ),
  );
}
