// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { recoveryNotice, selectRecoverySource } from "./recovery-source.ts";

it("keeps histories at the full-summary threshold unchanged", () => {
  const text: string = "x".repeat(128_000);
  const result = selectRecoverySource(text, 1000);
  expect(result.omittedCodeUnits).toBe(0);
  expect(result.text.length).toBe(128_000);
  expect(result.text === text).toBe(true);
});

it("keeps the beginning and a larger recent tail, with an explicit gap marker", () => {
  const result = selectRecoverySource(`BEGIN${"x".repeat(200_000)}END`, 1000);
  expect(result.text).toMatch(/^BEGIN[\s\S]*historical middle omitted[\s\S]*END$/u);
  expect(result.omittedCodeUnits).toBe(192_008);
  expect(result.text.length).toBeLessThan(9000);
});

it("never splits surrogate pairs at selected boundaries", () => {
  const result = selectRecoverySource(
    `${"a".repeat(1999)}😀${"x".repeat(130_000)}😀${"b".repeat(5999)}`,
    1000,
  );
  expect(result.omittedCodeUnits).toBe(130_004);
  expect(/[\uD800-\uDFFF]/u.exec(result.text)).toBeNull();
  expect(result.text).toMatch(/^a{1999}\n[\s\S]*\nb{5999}$/u);
});

it("makes omitted context and preservation explicit in the durable summary", () => {
  expect(recoveryNotice(192_008)).toBe(
    "Emergency recovery compaction: 192008 UTF-16 code units of historical middle were omitted from the summary input. The original session history is preserved. Verify missing requirements and decisions from that history before acting on them.",
  );
});
