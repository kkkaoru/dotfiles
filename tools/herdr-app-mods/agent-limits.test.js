import { afterEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import AgentLimits, {
  formatResetRemaining,
  parseAntigravity,
  parseColabUsage,
  parseGatewayCost,
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
// Trimmed from real `codexbar usage --provider codex --json`: the ChatGPT/Codex plan's monthly window.
const CODEX = JSON.stringify([
  {
    provider: "codex",
    usage: {
      primary: { windowMinutes: 43200, usedPercent: 61, resetsAt: "2026-11-25T12:25:27Z" },
      secondary: null,
      tertiary: null,
    },
  },
]);
const GO_ERROR = JSON.stringify([
  {
    provider: "opencodego",
    error: { code: 1, kind: "provider", message: "OpenCode Go API error: HTTP 500" },
  },
]);
// Grok's CLI proxy reports a percent and a reset without the window's length.
const GROK_NO_WINDOW = JSON.stringify([
  {
    provider: "grok",
    usage: { primary: { usedPercent: 100, resetsAt: "2026-11-17T07:35:47Z" }, secondary: null },
  },
]);
// Antigravity's quota pools, trimmed from real `codexbar usage --provider antigravity --json`:
// Gemini and Claude/GPT each have a 5h and a weekly window, Argon only a weekly one.
const antigravityWindow = (title, windowMinutes, usedPercent) => ({
  title,
  id: `antigravity-quota-summary-${title}`,
  window: { windowMinutes, usedPercent, resetsAt: "2026-11-21T14:59:37Z" },
});
const ANTIGRAVITY = JSON.stringify([
  {
    provider: "antigravity",
    usage: {
      primary: { windowMinutes: 300, usedPercent: 12.4 },
      secondary: { windowMinutes: 300, usedPercent: 0 },
      tertiary: null,
      extraRateWindows: [
        antigravityWindow("gemini-5h", 300, 12.4),
        antigravityWindow("gemini-weekly", 10080, 87.6),
        antigravityWindow("3p-5h", 300, 0),
        antigravityWindow("3p-weekly", 10080, 40.2),
        antigravityWindow("gemini-argon-wtu-weekly", 10080, 60),
      ],
    },
  },
]);

// `colab usage` as the CLI prints it for an account with compute units left.
const COLAB = "Current balance: 194.09 compute units\nUsage rate: 5.38/hr";

// `scripts/gateway-cost.sh` prints the gateway's own dollars for today.
const GATEWAY = "18.225484842588916\n";

const ANSWERS = {
  claude: CLAUDE,
  codex: CODEX,
  ollama: OLLAMA,
  opencodego: GO_ERROR,
  grok: GROK,
  antigravity: ANTIGRAVITY,
  colab: COLAB,
  gateway: GATEWAY,
};

// Answers exec by the provider asked for; a cx whose spawned tasks and timer a test runs by hand.
function newLimits(answers) {
  host.exec = async (command, args) => {
    if (command !== "sh") {
      return { code: 0, stdout: answers[args[2]], stderr: "" };
    }
    const scripted =
      ["opencodego", "colab", "gateway"].find((name) => args[1].includes(name)) ?? "ollama";
    return { code: 0, stdout: answers[scripted], stderr: "" };
  };
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
  vi.useRealTimers();
  reset();
});

test("parseLimits reads each reported window and leaves out a placeholder", () => {
  expect(parseLimits(CLAUDE)).toStrictEqual([
    { label: "7d", used: 24, resetsAt: "2026-11-17T12:59:59Z" },
  ]);
});

test("parseLimits leaves the label out when the provider reports no window length", () => {
  expect(parseLimits(GROK_NO_WINDOW)).toStrictEqual([
    { label: null, used: 100, resetsAt: "2026-11-17T07:35:47Z" },
  ]);
});

test("parseOllamaBalance turns the remaining session and weekly shares into used percents", () => {
  expect(parseOllamaBalance(OLLAMA)).toStrictEqual([
    { label: "5h", used: 12, resetsAt: "2026-11-14T11:00:00Z" },
    { label: "7d", used: 85, resetsAt: "2026-11-16T00:00:00Z" },
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

test("parseAntigravity reads one window per quota pool (Gemini, Claude/GPT, Argon)", () => {
  expect(parseAntigravity(ANTIGRAVITY)).toStrictEqual([
    { label: "💧7d", used: 88, resetsAt: "2026-11-21T14:59:37Z", keep: true },
    { label: "🤖7d", used: 40, resetsAt: "2026-11-21T14:59:37Z", keep: true },
    { label: "🅰️7d", used: 60, resetsAt: "2026-11-21T14:59:37Z", keep: true },
  ]);
});

test("parseAntigravity leaves a placeholder window out and is null on an error", () => {
  const placeholder = JSON.stringify([
    {
      usage: {
        extraRateWindows: [
          { title: "gemini-5h", window: { windowMinutes: 300, isSyntheticPlaceholder: true } },
          { title: "gemini-weekly", window: { windowMinutes: 10080, usedPercent: 30 } },
        ],
      },
    },
  ]);
  expect(parseAntigravity(placeholder)).toStrictEqual([{ label: "💧7d", used: 30, keep: true }]);
  expect(parseAntigravity(GO_ERROR)).toBe(null);
});

test("parseAntigravity has no windows when the account reported no pools", () => {
  expect(parseAntigravity(JSON.stringify([{ usage: {} }]))).toStrictEqual([]);
});

test("parseLimits throws on output that is not CodexBar's JSON", () => {
  expect(() => parseLimits("Segmentation fault")).toThrow();
});

test("init asks each provider once, Ollama from its balance API and OpenCode Go from its usage API, and polls every five minutes", async () => {
  const { cx, tasks, timers } = newLimits(ANSWERS);
  await Promise.all(tasks);
  expect(host.calls).toStrictEqual([
    ["exec", "codexbar", ["usage", "--provider", "claude", "--source", "oauth", "--json"]],
    ["exec", "codexbar", ["usage", "--provider", "codex", "--source", "auto", "--json"]],
    [
      "exec",
      "sh",
      [
        "-c",
        'key="$OLLAMA_API_KEY"; for file in ~/.codex/.env ~/.env; do [ -n "$key" ] || key=$(sed -n "s/^OLLAMA_API_KEY=//p" "$file" 2>/dev/null | head -n 1 | tr -d "\\"\'"); done; [ -n "$key" ] || key=$(plutil -extract ollama-cloud.key raw -o - ~/.pi/agent/auth.json 2>/dev/null); printf "Authorization: Bearer %s\\n" "$key" | curl --silent --fail --max-time 20 -H @- https://ollama.com/api/balance',
      ],
    ],
    [
      "exec",
      "sh",
      [
        "-c",
        'export OPENCODE_API_KEY="${OPENCODE_API_KEY:-$(plutil -extract opencode-go.key raw -o - ~/.pi/agent/auth.json 2>/dev/null)}"; codexbar usage --provider opencodego --source api --json',
      ],
    ],
    ["exec", "codexbar", ["usage", "--provider", "grok", "--source", "auto", "--json"]],
    ["exec", "codexbar", ["usage", "--provider", "antigravity", "--source", "auto", "--json"]],
    ["exec", "sh", ["-c", '"${COLAB_CLI:-$HOME/.local/bin/colab}" --auth=adc usage']],
    [
      "exec",
      "sh",
      ["-c", '"$HOME/ghq/github.com/kkkaoru/dotfiles/scripts/gateway-cost.sh" 2>/dev/null'],
    ],
  ]);
  expect(cx.notify).toHaveBeenCalledTimes(8);
  expect(timers.map(([ms]) => ms)).toStrictEqual([300000]);
  timers[0][1](cx);
  await Promise.all(tasks);
  expect(host.calls.length).toBe(16);
});

test("render shows each provider's windows, a dash for one that failed or has not answered", async () => {
  const { view, tasks } = newLimits(ANSWERS);
  expect(drawn(view)).toStrictEqual([
    ["icons/claude.svg", "—"],
    ["icons/codex.svg", "—"],
    ["icons/ollama.svg", "—"],
    ["icons/opencode.svg", "—"],
    ["icons/grok.svg", "—"],
    ["icons/antigravity.svg", "—"],
    [undefined, "—"],
    [undefined, "—"],
  ]);
  await Promise.all(tasks);
  expect(drawn(view)).toStrictEqual([
    ["icons/claude.svg", "7d24%"],
    ["icons/codex.svg", "61%"],
    ["icons/ollama.svg", "5h12%"],
    ["icons/opencode.svg", "—"],
    ["icons/grok.svg", "7d0%"],
    ["icons/antigravity.svg", "💧7d88%", "🤖7d40%", "🅰️7d60%"],
    [undefined, "194CU"],
    [undefined, "$18.23/day"],
  ]);
});

test("render shows a window without a length as its percent alone, and shows the reset countdown without the window's length from 95%", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-11-16T05:10:00Z"));
  const { view, tasks } = newLimits({ ...ANSWERS, grok: GROK_NO_WINDOW });
  await Promise.all(tasks);
  expect(drawn(view)[4]).toStrictEqual(["icons/grok.svg", "1d"]);

  const fullClaude = JSON.stringify([
    {
      provider: "claude",
      usage: {
        primary: { windowMinutes: 300, usedPercent: 100, resetsAt: "2026-11-16T08:15:00Z" },
      },
    },
  ]);
  const fullView = newLimits({ ...ANSWERS, claude: fullClaude });
  await Promise.all(fullView.tasks);
  expect(drawn(fullView.view)[0]).toStrictEqual(["icons/claude.svg", "3h"]);

  const nearlyFullClaude = JSON.stringify([
    {
      provider: "claude",
      usage: {
        primary: { windowMinutes: 10080, usedPercent: 96, resetsAt: "2026-11-18T08:15:00Z" },
      },
    },
  ]);
  const nearlyFullView = newLimits({ ...ANSWERS, claude: nearlyFullClaude });
  await Promise.all(nearlyFullView.tasks);
  expect(drawn(nearlyFullView.view)[0]).toStrictEqual(["icons/claude.svg", "2d"]);
});

test("formatResetRemaining formats days, hours, or minutes (coarsest unit) or returns null when missing/expired", () => {
  const now = Date.parse("2026-11-16T00:00:00Z");
  expect(formatResetRemaining(undefined, now)).toBe(null);
  expect(formatResetRemaining("not-a-date", now)).toBe(null);
  expect(formatResetRemaining("2026-11-15T23:59:00Z", now)).toBe(null);
  expect(formatResetRemaining("2026-11-16T00:45:00Z", now)).toBe("45m");
  expect(formatResetRemaining("2026-11-16T03:20:00Z", now)).toBe("3h");
  expect(formatResetRemaining("2026-11-18T02:05:00Z", now)).toBe("2d");
});

test("a window at 80% or more is drawn in the warning color", async () => {
  const hot = JSON.stringify({ included: { session: { remaining_percent: 20 } } });
  const { view, tasks } = newLimits({ ...ANSWERS, ollama: hot });
  await Promise.all(tasks);
  const [claude, codex, ollama] = view.render({}).kids.map((group) => group.kids[1]);
  expect([claude.calls, ollama.calls]).toStrictEqual([
    [["text_color", "#f"]],
    [["text_color", "#r"]],
  ]);
  expect(codex.calls).toStrictEqual([["text_color", "#f"]]);
});

test("a hundred-hour window is labelled in hours", () => {
  const answer = JSON.stringify([{ usage: { primary: { windowMinutes: 6000, usedPercent: 1 } } }]);
  expect(parseLimits(answer)).toStrictEqual([{ label: "100h", used: 1 }]);
});

test("parseColabUsage reads the compute-unit balance, or is null when it could not be read", () => {
  expect(parseColabUsage(COLAB)).toStrictEqual([{ text: "194CU" }]);
  expect(parseColabUsage("[colab] Warning: failed to fetch compute-unit info")).toBe(null);
});

test("the two cloud rows carry their own tag", () => {
  const { view } = newLimits(ANSWERS);
  const tags = view
    .render({})
    .kids.slice(-2)
    .map((group) => group.kids[0].kids);
  expect(tags).toStrictEqual([["📓"], ["☁️"]]);
});

test("parseGatewayCost reads the day's dollars, or is null when the script printed none", () => {
  expect(parseGatewayCost(GATEWAY)).toStrictEqual([{ text: "$18.23/day" }]);
  expect(parseGatewayCost("0\n")).toStrictEqual([{ text: "$0.00/day" }]);
  expect(parseGatewayCost("")).toBe(null);
});

test("the monthly window omits its 30d label", () => {
  const answer = JSON.stringify([
    { usage: { primary: { windowMinutes: 43200, usedPercent: 61 }, secondary: null } },
  ]);
  expect(parseLimits(answer)).toStrictEqual([{ label: null, used: 61 }]);
});

test("Claude's icon keeps its own colors, the one-color glyphs take the text color", () => {
  const { view } = newLimits(ANSWERS);
  const [claude, codex, ollama] = view.render({}).kids.map((group) => group.kids[0]);
  expect([claude.kind, claude.calls]).toStrictEqual([
    "image",
    [["w", 14], ["h", 14], ["flex_none"]],
  ]);
  expect([codex.kind, codex.calls]).toStrictEqual(["image", [["w", 14], ["h", 14], ["flex_none"]]]);
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
  expect(drawn(view)[3]).toStrictEqual(["icons/opencode.svg", "5h0%"]);
});
