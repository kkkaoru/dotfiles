// This TypeScript file is executed with Bun.
import { wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { expect, test } from "vitest";
import { fitting, problems, type Render } from "./fit.ts";
import { start } from "./harness.ts";

/** Widths a window is likely to have, from unusably narrow to a wide terminal. */
const WIDTHS = [8, 12, 16, 20, 24, 32, 40, 60, 80, 120, 200];

const TITLE =
  "TikTok replay 7710554634280192774 reaches the verified stream (34/34, edit_completion passes)";
const NOTE =
  "Reached 31/34. youtube-master done, three steps left: clips, web-hls, speech-edit. Progressing without a stall since the missing binary was fixed.";

/** A renderer that wraps, as pi does with a plain string line. */
const wrapping: Render = (lines, width) => lines.flatMap((line) => wrapTextWithAnsi(line, width));

/** A renderer that swaps characters of a line, as a mangled row would. */
const mangling: Render = (lines) => lines.map((line) => line.replace("own", "won"));

/** A renderer that ignores the width and returns the line as it is. */
const spilling: Render = (lines) => lines;

test("no width makes the widget wrap, merge or mangle a row", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: TITLE });
  await harness.run({ action: "update", id: "t1", status: "active", note: NOTE });
  // A width nothing needs to cut at, so these are the widget's own lines, note and all.
  const lines = harness.widgetLines(4000);
  for (const width of WIDTHS) {
    expect({ width, found: problems(lines, width) }).toStrictEqual({ found: [], width });
  }
});

test("the detector reports the rows pi would wrap", () => {
  const found = problems([TITLE, NOTE], 20, wrapping);
  expect(found.some((problem) => problem.detail.includes("rows for"))).toBe(true);
});

test("the detector reports a row that loses characters of its own line", () => {
  expect(problems(["a line of its own"], 40, mangling)).toStrictEqual([
    { detail: "does not begin its own line: a line of its won", row: 0 },
  ]);
});

test("the detector reports a row wider than the width it is drawn at", () => {
  expect(problems(["a row that is far too wide"], 10, spilling)).toStrictEqual([
    { detail: "26 wide at 10", row: 0 },
  ]);
});

test("a drawn row needs no invalidation", () => {
  // The rows are recomputed from the lines on every render, so there is nothing cached to drop.
  expect(fitting(["a line"]).invalidate()).toBeUndefined();
});
