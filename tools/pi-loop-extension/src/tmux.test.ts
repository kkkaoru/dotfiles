// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { detachBashInTmux } from "./tmux.ts";

it("wraps a long-timeout command in a detached tmux session", () => {
  const input = { command: "gh run watch 32847265628 --exit-status --compact", timeout: 1200 };

  expect(detachBashInTmux(input, 7)).toBe(true);
  expect(input.timeout).toBe(30);
  expect(input.command).toMatch(/tmux new-session -d -s 'pi-loop-\d+-7'/u);
  expect(input.command).toMatch(/output\.log/u);
  expect(input.command).toMatch(/exit-status/u);
  expect(input.command).toMatch(/Schedule a loop_wakeup/u);
});

it("wraps known watch commands without an explicit timeout", () => {
  const input: { command: string; timeout?: number } = { command: "tail -f 'server log'" };

  expect(detachBashInTmux(input, 1)).toBe(true);
  expect(input.timeout).toBe(30);
  expect(input.command).toMatch(/tail -f '"'"'server log'"'"'/u);
});

it("does not wrap short or existing tmux commands", () => {
  expect(detachBashInTmux({ command: "bun test", timeout: 30 }, 1)).toBe(false);
  expect(
    detachBashInTmux(
      { command: "tmux new-session -d -s existing 'gh run watch 1'", timeout: 1200 },
      2,
    ),
  ).toBe(false);
});
