// This TypeScript file is executed with Bun.
import { homedir } from "node:os";
import path from "node:path";
import type {
  ExtensionAPI,
  ExtensionContext,
  ReadonlyFooterDataProvider,
  Theme,
} from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { expect, test, vi } from "vitest";
import compactFooter, {
  contextText,
  folderOf,
  footerLines,
  shortModel,
  shortSession,
  shortStatus,
  twoEnds,
} from "./index.ts";

const theme: Theme = { fg: (_color: string, text: string) => text } as unknown as Theme;

function data(
  statuses: readonly (readonly [string, string])[],
  branch: string | null = "main",
): ReadonlyFooterDataProvider {
  return {
    getGitBranch: () => branch,
    getExtensionStatuses: () => new Map(statuses),
    getAvailableProviderCount: () => 1,
    onBranchChange: () => () => {},
  };
}

function ctx(
  usage: { percent: number | null; contextWindow: number } | undefined,
): ExtensionContext {
  return {
    cwd: path.join(homedir(), "ghq", "github.com", "kkkaoru", "dotfiles"),
    model: { id: "workers-ai/@cf/deepseek-ai/deepseek-v4.1-flash-1017" },
    getContextUsage: () => usage,
    sessionManager: { getSessionId: () => "01a1eefb-96d0-7357-8910-7b985ea3ce84" },
  } as unknown as ExtensionContext;
}

test("the folder names the checkout, an outside path stays whole", () => {
  expect(folderOf(path.join(homedir(), "ghq", "dotfiles"))).toBe("dotfiles");
  expect(folderOf("/Volumes/ssd/work")).toBe("/Volumes/ssd/work");
});

test("the model keeps its own name, without the provider path", () => {
  expect(shortModel("workers-ai/@cf/deepseek-ai/deepseek-v4.1-flash-1017")).toBe(
    "deepseek-v4.1-flash-1017",
  );
  expect(shortModel(undefined)).toBe("");
});

test("eight characters are enough to resume the session", () => {
  expect(shortSession("01a1eefb-96d0-7357-8910-7b985ea3ce84")).toBe("01a1eefb");
});

test("ponytail's label is compressed, other packages' text is left alone", () => {
  expect(shortStatus("ponytail", "● 🐴 ponytail: ⚡ FULL")).toBe("🐴 ⚡ FULL");
  expect(shortStatus("ponytail", "● 🐴 something else")).toBe("● 🐴 something else");
  expect(shortStatus("gateway-cost", "gateway $29.8/24h")).toBe("gateway $29.8/24h");
});

test("context usage is a percentage of the window, or the window alone", () => {
  expect(contextText({ percent: 30.24, contextWindow: 262_144 })).toBe("30.2%/262k");
  expect(contextText({ percent: null, contextWindow: 800 })).toBe("800");
  expect(contextText(undefined)).toBe("");
});

test("both ends of a row stay on one line", () => {
  expect(twoEnds("left", "right", 20)).toBe(`left${" ".repeat(11)}right`);
  expect(stripTerminalSequences(twoEnds("left", "right", 10))).toBe(`left${" ".repeat(2)}righ`);
  expect(stripTerminalSequences(twoEnds("a long left side", "right", 6))).toBe("a long");
});

test("the footer is two short rows with the session id in the corner", () => {
  const lines = footerLines(
    ctx({ percent: 30.24, contextWindow: 262_144 }),
    data([
      ["gateway-cost", "gateway $29.8/24h"],
      ["pi-effort-thinking", "think:max · dynamic"],
      ["ponytail", "● 🐴 ponytail: ⚡ FULL"],
      ["tmux-timeout", "3 jobs"],
    ]),
    theme,
    100,
  );
  expect(lines[0]).toBe(`dotfiles (main)${" ".repeat(61)}deepseek-v4.1-flash-1017`);
  // The unlisted status (tmux-timeout) is what pi's own footer used to overflow the row with.
  expect(
    lines[1]?.startsWith("30.2%/262k · gateway $29.8/24h · think:max · dynamic · 🐴 ⚡ FULL"),
  ).toBe(true);
  expect(lines[1]?.endsWith("01a1eefb")).toBe(true);
  expect(lines.map((line) => visibleWidth(line))).toStrictEqual([100, 100]);
});

test("a row without statuses still shows the folder and the session", () => {
  const lines = footerLines(ctx(undefined), data([], null), theme, 40);
  expect(lines[0]).toBe(`dotfiles${" ".repeat(8)}deepseek-v4.1-flash-1017`);
  expect(lines[1]).toBe(`${" ".repeat(32)}01a1eefb`);
});

test("the session start installs the footer and its branch subscription", () => {
  const handlers = new Map<string, (event: unknown, ctx: ExtensionContext) => void>();
  const unsubscribe = vi.fn();
  const requestRender = vi.fn();
  const setFooter = vi.fn();
  compactFooter({
    on: (name: string, handler: (event: unknown, ctx: ExtensionContext) => void) => {
      handlers.set(name, handler);
    },
  } as unknown as ExtensionAPI);
  handlers.get("session_start")?.({}, {
    mode: "tui",
    cwd: path.join(homedir(), "ghq", "dotfiles"),
    model: { id: "a/b" },
    getContextUsage: () => undefined,
    sessionManager: { getSessionId: () => "01a1eefb-96d0" },
    ui: { setFooter },
  } as unknown as ExtensionContext);
  const factory = setFooter.mock.calls[0]?.[0] as (
    tui: unknown,
    theme: Theme,
    footerData: ReadonlyFooterDataProvider,
  ) => { dispose: () => void; invalidate: () => void; render: (width: number) => string[] };
  const component = factory({ requestRender }, theme, {
    ...data([]),
    onBranchChange: (callback: () => void) => {
      callback();
      return unsubscribe;
    },
  });
  component.invalidate();
  expect(requestRender).toHaveBeenCalledTimes(1);
  expect(component.render(20)).toHaveLength(2);
  component.dispose();
  expect(unsubscribe).toHaveBeenCalledTimes(1);
});

test("a non-TUI mode keeps the built-in footer", () => {
  const handlers = new Map<string, (event: unknown, ctx: ExtensionContext) => void>();
  const setFooter = vi.fn();
  compactFooter({
    on: (name: string, handler: (event: unknown, ctx: ExtensionContext) => void) => {
      handlers.set(name, handler);
    },
  } as unknown as ExtensionAPI);
  handlers.get("session_start")?.({}, {
    mode: "rpc",
    ui: { setFooter },
  } as unknown as ExtensionContext);
  expect(setFooter).not.toHaveBeenCalled();
});
