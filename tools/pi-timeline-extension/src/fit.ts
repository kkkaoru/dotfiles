// This TypeScript file is executed with Bun.
import {
  stripTerminalSequences,
  truncateToWidth,
  visibleWidth,
  type Component,
} from "@earendil-works/pi-tui";

const ELLIPSIS = "\u2026";

/** Rows the widget draws: one row per line, cut at the width so it never runs into the next. */
export function fitting(lines: readonly string[]): Component {
  return {
    invalidate: (): void => undefined,
    render: (width: number): readonly string[] =>
      lines.map((line) => truncateToWidth(line, Math.max(1, width), ELLIPSIS)),
  };
}

/** Turns lines into the rows drawn at a width; a test hands in a wrong one to prove detection. */
export type Render = (lines: readonly string[], width: number) => readonly string[];

/** One thing wrong with the widget as it would be drawn. */
export interface Problem {
  /** The row it was seen at, or the count when the rows do not line up with the lines. */
  readonly row: number;
  readonly detail: string;
}

/** The rows pi would draw for `lines` at `width`. */
const draw = (lines: readonly string[], width: number): readonly string[] =>
  fitting(lines).render(width);

/**
 * What is wrong with the widget drawn at `width`: a row wider than the width, which pi would wrap
 * and push the tree around; a row count that does not match the lines, so a row wrapped or merged;
 * or a row whose own characters went missing, other than the ellipsis that marks a cut tail.
 */
export function problems(
  lines: readonly string[],
  width: number,
  render: Render = draw,
): readonly Problem[] {
  const rows = render(lines, width);
  const found: Problem[] = [];
  if (rows.length !== lines.length) {
    found.push({
      row: rows.length,
      detail: `${String(rows.length)} rows for ${String(lines.length)} lines`,
    });
  }
  for (const [index, row] of rows.entries()) {
    const drawn = visibleWidth(row);
    if (drawn > width) {
      found.push({ row: index, detail: `${String(drawn)} wide at ${String(width)}` });
    }
    const source = stripTerminalSequences(lines[index] ?? "");
    const kept = stripTerminalSequences(row).replace(new RegExp(`${ELLIPSIS}$`, "u"), "");
    if (!source.startsWith(kept)) {
      found.push({ row: index, detail: `does not begin its own line: ${row}` });
    }
  }
  return found;
}
