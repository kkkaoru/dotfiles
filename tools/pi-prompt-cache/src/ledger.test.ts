// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import {
  EMPTY_LEDGER,
  hitPercent,
  isMiss,
  parseLedger,
  record,
  renderLedger,
  ROW_LIMIT,
  statusText,
  totals,
  type Row,
} from "./ledger.ts";

const row = (prompt: number, cached: number, part: string | null = null): Row => ({
  at: Date.UTC(2026, 0, 1, 0, 0, 0),
  cached,
  model: "m",
  part,
  prompt,
});

test("the hit rate is the cached share of every prompt in the session", () => {
  const ledger = record(record(EMPTY_LEDGER, row(1000, 900)), row(1000, 500));
  expect(totals(ledger)).toStrictEqual({ cached: 1400, prompt: 2000 });
  expect(hitPercent(ledger)).toBe(70);
  expect(statusText(ledger)).toBe("cache 70%");
});

test("an empty ledger reports no hit rather than dividing by zero", () => {
  expect(hitPercent(EMPTY_LEDGER)).toBe(0);
  expect(renderLedger(EMPTY_LEDGER)).toStrictEqual(["No request recorded yet."]);
});

test("only the newest rows are kept", () => {
  let ledger = EMPTY_LEDGER;
  for (let index = 0; index < ROW_LIMIT + 5; index += 1) {
    ledger = record(ledger, row(10, index));
  }
  expect(ledger.rows).toHaveLength(ROW_LIMIT);
  expect(ledger.rows.at(-1)?.cached).toBe(ROW_LIMIT + 4);
});

test("a small prompt that was not cached is not a miss worth reporting", () => {
  expect(isMiss(row(1000, 0))).toBe(false);
  expect(isMiss(row(9000, 9000))).toBe(false);
  expect(isMiss(row(9000, 4600))).toBe(false);
  expect(isMiss(row(9000, 100))).toBe(true);
});

test("the report names the part that broke the prefix", () => {
  const miss = row(20_000, 400, "tools");
  const lines = renderLedger(record(record(EMPTY_LEDGER, miss), row(500, 400)));
  expect(lines[0]).toBe("4% cached · 2 requests · 20,500 prompt, 800 cached");
  expect(lines[1]).toBe("Large prompts the provider recomputed:");
  expect(lines[2]).toContain("20,000 prompt after the tools changed");
});

test("a miss with an unchanged prefix is reported without a part", () => {
  const lines = renderLedger(record(EMPTY_LEDGER, row(30_000, 0)));
  expect(lines[2]).toContain("30,000 prompt");
  expect(lines[2]).not.toContain("after the");
});

test("a report with no miss says so", () => {
  const hits = record(EMPTY_LEDGER, row(20_000, 19_000));
  expect(renderLedger(hits)).toStrictEqual([
    "95% cached · 1 requests · 20,000 prompt, 19,000 cached",
    "No large prompt was recomputed.",
  ]);
});

test("a stored ledger is read back, and anything unreadable is empty", () => {
  const ledger = record(EMPTY_LEDGER, row(1000, 500, "system"));
  expect(parseLedger(JSON.stringify(ledger))).toStrictEqual(ledger);
  expect(parseLedger("")).toStrictEqual(EMPTY_LEDGER);
  expect(parseLedger("null")).toStrictEqual(EMPTY_LEDGER);
  expect(parseLedger("[]")).toStrictEqual(EMPTY_LEDGER);
  expect(parseLedger('{"rows":"x"}')).toStrictEqual(EMPTY_LEDGER);
  expect(parseLedger('{"rows":[null,"x"]}')).toStrictEqual(EMPTY_LEDGER);
  const stored = JSON.stringify({
    rows: [{ at: 1 }, { at: 1, cached: 1, model: "m", part: "x", prompt: 1 }],
  });
  const expected: Row = { at: 1, cached: 1, model: "m", part: "x", prompt: 1 };
  expect(parseLedger(stored)).toStrictEqual({ rows: [expected] });
});
