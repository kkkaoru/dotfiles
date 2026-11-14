import { afterEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import AgentLimits, {
  parseLimits,
  parseOllamaBalance,
  shownWindows,
} from "../../.config/herdr-app/mods/agent-limits/main.js";

// Trimmed from real `codexbar usage --provider <p> --json` answers, account fields left out.
const CLAUDE = JSON.stringify([
  {
    provider: "claude",
    usage: {
      primary: { windowMinutes: 300, isSyntheticPlaceholder: true, usedPercent: 0 },
      secondary: { windowMinutes: 10080, usedPercent: 24, resetsAt: "2026-11-17T12:59:59Z" },
      tertiary: null,
    },
  },
]);
const GROK = JSON.stringify([
  {
    provider: "grok",
    usage: { primary: { windowMinutes: 10080, usedPercent: 0 }, secondary: null, tertiary: null },
  },
]);
// Ollama's /api/balance as it answers today.
const OLLAMA = JSON.stringify({
  included: {
    session: { remaining_percent: 87.6, resets_at: "2026-11-14T11:00:00Z" },
    weekly: { remaining_percent: 15, resets_at: "2026-11-16T00:00:00Z" },
  },
  purchased: { balance_usd: 0 },
});
const GO_ERROR = JSON.stringify([
  {
    provider: "opencodego",
    error: { code: 1, kind: "provider", message: "OpenCode Go API error: HTTP 500" },
  },
]);

const ANSWERS = { claude: CLAUDE, ollama: OLLAMA, opencodego: GO_ERROR, grok: GROK };

// Answers exec by the provider asked for; a cx whose spawned tasks and timer a test runs by hand.
function newLimits(answers) {
  host.exec = async (command, args) => ({
    code: 0,
    stdout: command === "sh" ? answers.ollama : answers[args[2]],
    stderr: "",
  });
  host.theme = { dim: "#d", foreground: "#f", blocked: "#r", chrome: "#c", border: "#b" };
  const tasks = [];
  const timers = [];
  const cx = {
    notify: vi.fn(),
    spawn: (fn) => tasks.push(fn(cx)),
    timer: { every: (ms, fn) => timers.push([ms, fn]) },
  };
  const view = new AgentLimits();
  view.init({}, cx);
  return { view, cx, tasks, timers };
}

// Each provider group as drawn: its icon's path, then each window's text or a dash.
const drawn = (view) =>
  view
    .render({})
    .kids.map(({ kids }) => kids.map((part, index) => (index === 0 ? part.path : part.kids[0])));

afterEach(() => {
  reset();
});

test("parseLimits reads each reported window and leaves out a placeholder", () => {
  expect(parseLimits(CLAUDE)).toStrictEqual([{ label: "7d", used: 24 }]);
});

test("parseOllamaBalance turns the remaining session and weekly shares into used percents", () => {
  expect(parseOllamaBalance(OLLAMA)).toStrictEqual([
    { label: "5h", used: 12 },
    { label: "7d", used: 85 },
  ]);
  expect(
    parseOllamaBalance(JSON.stringify({ included: { weekly: { remaining_percent: 0 } } })),
  ).toStrictEqual([{ label: "7d", used: 100 }]);
});

test("parseOllamaBalance is null when curl got no answer", () => {
  expect(parseOllamaBalance("")).toBe(null);
});

test("parseLimits is null when the provider answered with an error", () => {
  expect(parseLimits(GO_ERROR)).toBe(null);
});

test("parseLimits throws on output that is not CodexBar's JSON", () => {
  expect(() => parseLimits("Segmentation fault")).toThrow();
});

test("init asks each provider once, Ollama from its balance API and OpenCode Go from its usage API, and polls every five minutes", async () => {
  const { cx, tasks, timers } = newLimits(ANSWERS);
  await Promise.all(tasks);
  expect(host.calls).toStrictEqual([
    ["exec", "codexbar", ["usage", "--provider", "claude", "--source", "auto", "--json"]],
    [
      "exec",
      "sh",
      [
        "-c",
        'key="$OLLAMA_API_KEY"; for file in ~/.codex/.env ~/.env; do [ -n "$key" ] || key=$(sed -n "s/^OLLAMA_API_KEY=//p" "$file" 2>/dev/null | head -n 1 | tr -d "\\"\'"); done; printf "Authorization: Bearer %s\\n" "$key" | curl --silent --fail --max-time 20 -H @- https://ollama.com/api/balance',
      ],
    ],
    ["exec", "codexbar", ["usage", "--provider", "opencodego", "--source", "api", "--json"]],
    ["exec", "codexbar", ["usage", "--provider", "grok", "--source", "auto", "--json"]],
  ]);
  expect(cx.notify).toHaveBeenCalledTimes(4);
  expect(timers.map(([ms]) => ms)).toStrictEqual([300000]);
  timers[0][1](cx);
  await Promise.all(tasks);
  expect(host.calls.length).toBe(8);
});

test("render shows each provider's windows, a dash for one that failed or has not answered", async () => {
  const { view, tasks } = newLimits(ANSWERS);
  expect(drawn(view)).toStrictEqual([
    ["icons/claude.svg", "—"],
    ["icons/ollama.svg", "—"],
    ["icons/opencode.svg", "—"],
    ["icons/grok.svg", "—"],
  ]);
  await Promise.all(tasks);
  expect(drawn(view)).toStrictEqual([
    ["icons/claude.svg", "7d 24%"],
    ["icons/ollama.svg", "5h 12%"],
    ["icons/opencode.svg", "—"],
    ["icons/grok.svg", "7d 0%"],
  ]);
});

test("a window at 80% or more is drawn in the warning color", async () => {
  const hot = JSON.stringify({ included: { session: { remaining_percent: 20 } } });
  const { view, tasks } = newLimits({ ...ANSWERS, ollama: hot });
  await Promise.all(tasks);
  const [claude, ollama] = view.render({}).kids.map((group) => group.kids[1]);
  expect([claude.calls, ollama.calls]).toStrictEqual([
    [["text_color", "#f"]],
    [["text_color", "#r"]],
  ]);
});

test("a hundred-hour window is labelled in hours", () => {
  const answer = JSON.stringify([{ usage: { primary: { windowMinutes: 6000, usedPercent: 1 } } }]);
  expect(parseLimits(answer)).toStrictEqual([{ label: "100h", used: 1 }]);
});

test("Claude's icon keeps its own colors, the one-color glyphs take the text color", () => {
  const { view } = newLimits(ANSWERS);
  const [claude, ollama] = view.render({}).kids.map((group) => group.kids[0]);
  expect([claude.kind, claude.calls]).toStrictEqual([
    "image",
    [["w", 14], ["h", 14], ["flex_none"]],
  ]);
  expect([ollama.kind, ollama.calls]).toStrictEqual([
    "svg",
    [["size", 14], ["flex_none"], ["text_color", "#f"]],
  ]);
});

test("shownWindows shows only the shortest window while none is used up", () => {
  expect(
    shownWindows([
      { label: "5h", used: 0 },
      { label: "7d", used: 22 },
      { label: "30d", used: 61 },
    ]),
  ).toStrictEqual([{ label: "5h", used: 0 }]);
});

test("shownWindows shows only a used-up weekly window, not the hours", () => {
  expect(
    shownWindows([
      { label: "5h", used: 40 },
      { label: "7d", used: 100 },
      { label: "30d", used: 61 },
    ]),
  ).toStrictEqual([{ label: "7d", used: 100 }]);
});

test("shownWindows shows only the longest used-up window", () => {
  expect(
    shownWindows([
      { label: "5h", used: 100 },
      { label: "7d", used: 100 },
      { label: "30d", used: 100 },
    ]),
  ).toStrictEqual([{ label: "30d", used: 100 }]);
});

test("shownWindows keeps a lone window and an empty list as they are", () => {
  expect(shownWindows([{ label: "7d", used: 24 }])).toStrictEqual([{ label: "7d", used: 24 }]);
  expect(shownWindows([])).toStrictEqual([]);
});

test("a provider's windows are drawn as shownWindows leaves them", async () => {
  const goMonthly = JSON.stringify([
    {
      provider: "opencodego",
      usage: {
        primary: { windowMinutes: 300, usedPercent: 0 },
        secondary: { windowMinutes: 10080, usedPercent: 22 },
        tertiary: { windowMinutes: 43200, usedPercent: 61 },
      },
    },
  ]);
  const { view, tasks } = newLimits({ ...ANSWERS, opencodego: goMonthly });
  await Promise.all(tasks);
  expect(drawn(view)[2]).toStrictEqual(["icons/opencode.svg", "5h 0%"]);
});
