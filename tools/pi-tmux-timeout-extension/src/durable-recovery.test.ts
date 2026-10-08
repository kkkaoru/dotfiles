// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import {
  nextSessionTmuxLaunchId,
  recoverSessionTmuxLaunches,
  TMUX_SESSION_ENTRY_TYPE,
  TMUX_DELIVERED_ENTRY_TYPE,
} from "./persistence.ts";
import { createTmuxLaunch } from "./tmux.ts";

it("reserves durable IDs even after delivery and ignores malformed delivery markers", () => {
  const launch = createTmuxLaunch({
    command: "echo ok",
    id: 7,
    namespace: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  });
  const record = { type: "custom", customType: TMUX_SESSION_ENTRY_TYPE, data: launch };
  const entries = [
    null,
    {},
    { customType: TMUX_DELIVERED_ENTRY_TYPE },
    { customType: TMUX_DELIVERED_ENTRY_TYPE, data: null },
    { customType: TMUX_DELIVERED_ENTRY_TYPE, data: "bad" },
    { customType: TMUX_DELIVERED_ENTRY_TYPE, data: {} },
    { customType: TMUX_DELIVERED_ENTRY_TYPE, data: { sessionName: 42 } },
    record,
    {
      type: "custom",
      customType: TMUX_DELIVERED_ENTRY_TYPE,
      data: { sessionName: launch.sessionName },
    },
  ];
  const operations = {
    exists: (file: string): boolean => !file.endsWith("completion-delivered"),
    readDirectory: (): readonly string[] => [],
    readFile: (): string => "",
    statBirthtime: (): number => 0,
    writeFile: vi.fn(),
  };
  expect(
    recoverSessionTmuxLaunches(entries, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", operations),
  ).toStrictEqual([]);
  expect(nextSessionTmuxLaunchId(entries, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")).toBe(8);
  expect(nextSessionTmuxLaunchId(entries, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")).toBe(1);
});
