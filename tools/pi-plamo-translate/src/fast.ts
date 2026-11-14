// This TypeScript file is executed with Bun.
import {
  fencedLines,
  translateMarkdown,
  translateProse,
  type Translator,
} from "../../claude-plamo-translate/hooks/translate.ts";

const MARKER = /^(\s*(?:[-*+]\s+|\d+[.)]\s+|#{1,6}\s+|>\s*)*)(.+)$/u;
const CHUNK_LINES = 2;

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

/** Two lines per request. A reply that merges lines falls back to one request per line. */
export async function translateFast(text: string, translate: Translator): Promise<string> {
  const lines = text.split("\n");
  if (lines.some((line) => line.trimStart().startsWith("|"))) {
    return translateMarkdown(text, translate);
  }
  const pieces = piecesOf(lines, fencedLines(lines));
  if (pieces.length === 0) {
    return text;
  }
  return stitch(
    lines,
    pieces,
    await translateBodies(
      pieces.map((piece) => piece.body),
      translate,
    ),
  );
}
