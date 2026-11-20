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
import compactFooter, { contextText, footerLines, modelText, twoEnds } from "./index.ts";

const SESSION = "01a1eefb-96d0-7357-8910-7b985ea3ce84";
const MODEL = "workers-ai/@cf/deepseek-ai/deepseek-v4.1-flash-1017";
const MODEL_TEXT = "workers-ai/deepseek-v4.1-flash-1017";
const theme: Theme = { fg: (_color: string, text: string) => text } as unknown as Theme;
// What a row of the given width reads with its left and right text at the two ends.
const row = (left: string, right: string, width: number) =>
  left + " ".repeat(width - left.length - right.length) + right;

function data(statuses: readonly (readonly [string, string])[]): ReadonlyFooterDataProvider {
  return {
    getGitBranch: () => "main",
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
    model: { provider: "workers-ai", id: MODEL },
    getContextUsage: () => usage,
    sessionManager: { getSessionId: () => SESSION },
  } as unknown as ExtensionContext;
}

test("the model keeps its own name and the provider that routes it", () => {
  expect(modelText("workers-ai", MODEL)).toBe("workers-ai/deepseek-v4.1-flash-1017");
  expect(modelText(undefined, "anthropic/claude-opus-4")).toBe("claude-opus-4");
  expect(modelText("workers-ai", undefined)).toBe("");
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

test("the footer is two short rows: model above, whole session id below", () => {
  const lines = footerLines(
    ctx({ percent: 30.24, contextWindow: 262_144 }),
    data([
      ["gateway-cost", "gateway $8.9/24h"],
      ["pi-effort-thinking", "think:max · dynamic"],
      ["ponytail", "● 🐴 ponytail: ⚡ FULL"],
      ["tmux-timeout", "3 jobs"],
    ]),
    theme,
    100,
  );
  // The folder, its branch, the effort level, the gateway price and the ponytail mode are gone;
  // the session id is not shortened.
  expect(lines[0]).toBe(row("", MODEL_TEXT, 100));
  expect(stripTerminalSequences(lines[1] ?? "")).toBe(row("30.2%/262k", SESSION, 100));
  expect(lines.map((line) => visibleWidth(line))).toStrictEqual([100, 100]);
});

test("a status that repeats a reading already on the row takes no second place", () => {
  const lines = footerLines(
    ctx({ percent: 45.6, contextWindow: 262_144 }),
    data([["strata", "45.6%/262k"]]),
    theme,
    100,
  );
  expect(stripTerminalSequences(lines[1] ?? "")).toBe(row("45.6%/262k", SESSION, 100));
});

test("a row without statuses still shows the model and the session", () => {
  const lines = footerLines(ctx(undefined), data([]), theme, 60);
  expect(lines[0]).toBe(row("", MODEL_TEXT, 60));
  expect(lines[1]).toBe(row("", SESSION, 60));
});

test("the session start installs the footer, which draws on demand", () => {
  const handlers = new Map<string, (event: unknown, ctx: ExtensionContext) => void>();
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
    model: { provider: "a", id: "a/b" },
    getContextUsage: () => undefined,
    sessionManager: { getSessionId: () => SESSION },
    ui: { setFooter },
  } as unknown as ExtensionContext);
  const factory = setFooter.mock.calls[0]?.[0] as (
    tui: unknown,
    theme: Theme,
    footerData: ReadonlyFooterDataProvider,
  ) => { dispose: () => void; invalidate: () => void; render: (width: number) => string[] };
  const component = factory({ requestRender }, theme, data([]));
  component.invalidate();
  expect(component.render(20)).toHaveLength(2);
  component.dispose();
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
