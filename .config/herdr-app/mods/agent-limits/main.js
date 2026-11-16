// How much of each coding plan's usage limits is used, from CodexBar's CLI (`codexbar usage`) and
// Ollama's balance API, refreshed while the block is in the layout.

import { View, div, image, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, theme } from "herdr";

// CodexBar scrapes the providers' dashboards, which answer HTTP 429 when asked often.
const POLL_MS = 5 * 60 * 1000;
const WARN_PERCENT = 80;
const FULL_PERCENT = 100;
const ICON_SIZE = 14;
const MINUTES_PER_HOUR = 60;
const MINUTES_PER_DAY = 24 * MINUTES_PER_HOUR;
const DAYS_PER_MONTH = 30;
const WINDOW_KEYS = ["primary", "secondary", "tertiary"];
// Herdr.app started from the Dock lacks the shell's variables, so the key is read as envs.fish
// reads it: the variable, else ~/.codex/.env, else ~/.env (launchctl's copy can be stale), else
// pi's own login (~/.pi/agent/auth.json, which the dotfiles link keeps on every machine). printf
// is a shell builtin and curl reads the header from stdin, so the key is in no process's arguments.
const OLLAMA_BALANCE = [
  'key="$OLLAMA_API_KEY";',
  "for file in ~/.codex/.env ~/.env; do",
  '[ -n "$key" ] || key=$(sed -n "s/^OLLAMA_API_KEY=//p" "$file" 2>/dev/null | head -n 1 | tr -d "\\"\'");',
  "done;",
  '[ -n "$key" ] || key=$(plutil -extract ollama-cloud.key raw -o - ~/.pi/agent/auth.json 2>/dev/null);',
  'printf "Authorization: Bearer %s\\n" "$key" |',
  "curl --silent --fail --max-time 20 -H @- https://ollama.com/api/balance",
].join(" ");

// CodexBar reads OpenCode Go's usage API with OPENCODE_API_KEY, which lives in pi's login on this
// machine; the shell exports it so the key stays out of the arguments, as the Ollama one does.
const OPENCODE_GO_USAGE = [
  'export OPENCODE_API_KEY="${OPENCODE_API_KEY:-$(plutil -extract opencode-go.key raw -o - ~/.pi/agent/auth.json 2>/dev/null)}";',
  "codexbar usage --provider opencodego --source api --json",
].join(" ");

/** "5h", "7d", or a calendar for the monthly window (30d is wide); null when the provider reports
 *  no length (Grok's CLI proxy). */
const windowLabel = (minutes) => {
  if (!Number.isFinite(minutes)) {
    return null;
  }
  if (minutes === DAYS_PER_MONTH * MINUTES_PER_DAY) {
    return "📅";
  }
  return minutes % MINUTES_PER_DAY === 0
    ? `${minutes / MINUTES_PER_DAY}d`
    : `${Math.round(minutes / MINUTES_PER_HOUR)}h`;
};

/** The one window worth showing, from windows ordered shortest first: the longest used-up one,
 *  since the shorter ones no longer matter, else the shortest, which runs out first. */
export function shownWindows(windows) {
  const full = windows.findLast(({ used }) => used >= FULL_PERCENT);
  return full ? [full] : windows.slice(0, 1);
}

/** `codexbar usage --provider <p> --json` -> its windows as `{ label, used }`, or null when the
 *  provider answered with an error. A placeholder window (no data yet) is left out. */
export function parseLimits(stdout) {
  const [entry] = JSON.parse(stdout);
  if (entry.error) {
    return null;
  }
  return WINDOW_KEYS.map((key) => entry.usage[key])
    .filter((window) => window && !window.isSyntheticPlaceholder)
    .map((window) => ({
      label: windowLabel(window.windowMinutes),
      used: Math.round(window.usedPercent),
    }));
}

/** `codexbar usage --provider antigravity --json` -> its quota pools' windows as
 *  `{ label, used }`, the one closest to running out first, or null on an error. Antigravity splits
 *  quota into Gemini, Claude/GPT and Argon pools, so a single number can only show the tightest. */
export function parseAntigravity(stdout) {
  const [entry] = JSON.parse(stdout);
  if (entry.error) {
    return null;
  }
  return (entry.usage?.extraRateWindows ?? [])
    .map(({ window }) => window)
    .filter((window) => window && !window.isSyntheticPlaceholder)
    .map((window) => ({
      label: windowLabel(window.windowMinutes),
      used: Math.round(window.usedPercent),
    }))
    .sort((left, right) => right.used - left.used);
}

/** Ollama's `/api/balance` -> its session (5h) and weekly windows as `{ label, used }`, or null
 *  when curl got no answer (a rejected key or a server error). */
export function parseOllamaBalance(stdout) {
  if (stdout.trim() === "") {
    return null;
  }
  const { included } = JSON.parse(stdout);
  return [
    ["5h", included?.session],
    ["7d", included?.weekly],
  ]
    .filter(([, window]) => window)
    .map(([label, window]) => ({
      label,
      used: Math.round(FULL_PERCENT - window.remaining_percent),
    }));
}

const codexbar = (provider, source) => [
  "codexbar",
  ["usage", "--provider", provider, "--source", source, "--json"],
];

// [Herdr's agent icon, tinted (one-color glyph) or drawn as is, key, [command, args], parser].
// CodexBar rejects the balance Ollama's API answers today and its web path is redirected to sign
// in, so Ollama is asked directly; OpenCode Go's usage API needs its key exported, which Auto
// would instead look for in a browser session, and the console fails. Claude is asked for its
// OAuth meters: its Auto path falls back to the Claude CLI, which hangs here with no cookies.
const PROVIDERS = [
  ["icons/claude.svg", false, "claude", codexbar("claude", "oauth"), parseLimits],
  ["icons/codex.svg", false, "codex", codexbar("codex", "auto"), parseLimits],
  ["icons/ollama.svg", true, "ollama", ["sh", ["-c", OLLAMA_BALANCE]], parseOllamaBalance],
  ["icons/opencode.svg", true, "opencodego", ["sh", ["-c", OPENCODE_GO_USAGE]], parseLimits],
  ["icons/grok.svg", true, "grok", codexbar("grok", "auto"), parseLimits],
  ["icons/antigravity.svg", true, "antigravity", codexbar("antigravity", "auto"), parseAntigravity],
];

export default class AgentLimits extends View {
  init(_props, cx) {
    // Provider -> its windows, null after an error, absent before the first answer.
    this.limits = new Map();
    this.refresh(cx);
    cx.timer.every(POLL_MS, (cx) => this.refresh(cx));
  }

  // Each provider on its own: one that fails or hangs leaves the others' readings.
  refresh(cx) {
    for (const [, , provider, [command, args], parse] of PROVIDERS) {
      cx.spawn(async (cx) => {
        // CodexBar exits 1 with the error in its JSON, which parseLimits reads.
        const { stdout } = await exec(command, args);
        const limits = parse(stdout);
        this.limits.set(provider, limits && shownWindows(limits));
        cx.notify();
      });
    }
  }

  render(_cx) {
    const t = theme();
    const windowText = ({ label, used }) =>
      div()
        .text_color(used >= WARN_PERCENT ? t.blocked : t.foreground)
        .child(label ? `${label} ${used}%` : `${used}%`);
    // As agents-emoji draws an agent's icon from Herdr's program table.
    const icon = (path, tint) =>
      tint
        ? svg(path).size(ICON_SIZE).flex_none().text_color(t.foreground)
        : image(path).w(ICON_SIZE).h(ICON_SIZE).flex_none();
    const provider = ([path, tint, name]) => {
      const windows = this.limits.get(name);
      return h_flex()
        .gap(5)
        .items_center()
        .child(icon(path, tint))
        .children(windows?.length ? windows.map(windowText) : [div().text_color(t.dim).child("—")]);
    };
    return h_flex()
      .size_full()
      .px(14)
      .gap(14)
      .items_center()
      .justify_end()
      .bg(t.chrome)
      .border_t(1)
      .border_color(t.border)
      .text_size(11.8)
      .children(PROVIDERS.map(provider));
  }
}
