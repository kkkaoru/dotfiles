// How much of each coding plan's usage limits is used, from CodexBar's CLI (`codexbar usage`) and
// Ollama's balance API, refreshed while the block is in the layout.

import { View, div, image, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, theme } from "herdr";

// CodexBar scrapes the providers' dashboards, which answer HTTP 429 when asked often.
const POLL_MS = 5 * 60 * 1000;
const WARN_PERCENT = 80;
const FULL_PERCENT = 100;
const RESET_PERCENT = 95;
const ICON_SIZE = 14;
const MS_PER_MINUTE = 60 * 1000;
const MINUTES_PER_HOUR = 60;
const HOURS_PER_DAY = 24;
const MINUTES_PER_DAY = HOURS_PER_DAY * MINUTES_PER_HOUR;
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

// The Colab CLI's oauth2 strategy asks for an authorization code on stdin, which no spawned
// command can answer, so ask with Application Default Credentials as scripts/colab does.
const COLAB_USAGE = '"${COLAB_CLI:-$HOME/.local/bin/colab}" --auth=adc usage';
// The AI Gateway's own cost for today: the script keeps the account id encrypted and renews
// wrangler's hourly token, so this is the number the dashboard and pi's footer show.
const GATEWAY_COST = '"$HOME/ghq/github.com/kkkaoru/dotfiles/scripts/gateway-cost.sh" 2>/dev/null';

/** "5h", "7d", or null for 30d/missing length (Grok's CLI proxy). */
const DAYS_PER_MONTH = 30;

const windowLabel = (minutes) => {
  if (!Number.isFinite(minutes) || minutes === DAYS_PER_MONTH * MINUTES_PER_DAY) {
    return null;
  }
  return minutes % MINUTES_PER_DAY === 0
    ? `${minutes / MINUTES_PER_DAY}d`
    : `${Math.round(minutes / MINUTES_PER_HOUR)}h`;
};

/** Remaining time until `resetsAt` as `Xd`, `Yh`, or `Zm` (coarsest non-zero unit), or null. */
export function formatResetRemaining(resetsAt, now = Date.now()) {
  if (!resetsAt) {
    return null;
  }
  const target = Date.parse(resetsAt);
  if (!Number.isFinite(target)) {
    return null;
  }
  const totalMinutes = Math.max(0, Math.ceil((target - now) / MS_PER_MINUTE));
  if (totalMinutes === 0) {
    return null;
  }
  const days = Math.floor(totalMinutes / MINUTES_PER_DAY);
  if (days > 0) {
    return `${days}d`;
  }
  const hours = Math.floor(totalMinutes / MINUTES_PER_HOUR);
  if (hours > 0) {
    return `${hours}h`;
  }
  return `${totalMinutes}m`;
}

const withReset = (label, used, resetsAt) =>
  resetsAt ? { label, used, resetsAt } : { label, used };

/** The windows worth showing for a provider. Multi-pool providers (like Antigravity) already
 *  return one entry per pool; single-pool providers return their windows shortest-first so we pick
 *  the longest used-up one, else the shortest. */
export function shownWindows(windows) {
  if (windows.some(({ keep }) => keep)) {
    return windows.map(({ keep: _keep, ...window }) => window);
  }
  const full = windows.findLast(({ used }) => used >= FULL_PERCENT);
  return full ? [full] : windows.slice(0, 1);
}

/** `codexbar usage --provider <p> --json` -> its windows as `{ label, used, resetsAt? }`, or null
 *  when the provider answered with an error. A placeholder window (no data yet) is left out. */
export function parseLimits(stdout) {
  const [entry] = JSON.parse(stdout);
  if (entry.error) {
    return null;
  }
  return WINDOW_KEYS.map((key) => entry.usage[key])
    .filter((window) => window && !window.isSyntheticPlaceholder)
    .map((window) =>
      withReset(windowLabel(window.windowMinutes), Math.round(window.usedPercent), window.resetsAt),
    );
}

/** `codexbar usage --provider antigravity --json` -> one `{ label, used, resetsAt?, keep: true }`
 *  per Antigravity quota pool (Gemini, Claude/GPT, Argon), picking the full window in each pool if
 *  any, else the highest-used window in that pool, or null on an error. */
const ANTIGRAVITY_POOLS = [
  ["💧", (id) => id.includes("gemini-") && !id.includes("argon")],
  ["🤖", (id) => id.includes("3p-")],
  ["🅰️", (id) => id.includes("gemini-argon-")],
];

export function parseAntigravity(stdout) {
  const [entry] = JSON.parse(stdout);
  if (entry.error) {
    return null;
  }
  const windows = (entry.usage?.extraRateWindows ?? [])
    .filter(({ window }) => window && !window.isSyntheticPlaceholder)
    .map(({ id = "", title = "", window }) => ({
      id: id || title,
      minutes: window.windowMinutes,
      label: windowLabel(window.windowMinutes),
      used: Math.round(window.usedPercent),
      resetsAt: window.resetsAt,
    }))
    .sort((left, right) => left.minutes - right.minutes);
  return ANTIGRAVITY_POOLS.flatMap(([tag, match]) => {
    const matching = windows.filter((item) => match(item.id));
    if (matching.length === 0) {
      return [];
    }
    const full = matching.findLast(({ used }) => used >= FULL_PERCENT);
    const picked = full ?? [...matching].sort((left, right) => right.used - left.used)[0];
    return [
      {
        label: picked.label ? `${tag}${picked.label}` : tag,
        used: picked.used,
        ...(picked.resetsAt ? { resetsAt: picked.resetsAt } : {}),
        keep: true,
      },
    ];
  });
}

/** Ollama's `/api/balance` -> its session (5h) and weekly windows as `{ label, used, resetsAt? }`,
 *  or null when curl got no answer (a rejected key or a server error). */
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
    .map(([label, window]) =>
      withReset(label, Math.round(FULL_PERCENT - window.remaining_percent), window.resets_at),
    );
}

const codexbar = (provider, source) => [
  "codexbar",
  ["usage", "--provider", provider, "--source", source, "--json"],
];

/** `colab usage` -> the account's remaining compute units as a single `{ text }`, or null when the
 *  balance could not be read (no credentials, Colab error). */
export function parseColabUsage(stdout) {
  const balance = /Current balance:\s*([\d.]+)/u.exec(stdout)?.[1];
  return balance === undefined ? null : [{ text: `${Math.round(Number(balance))}CU` }];
}

/** `scripts/gateway-cost.sh` -> today's gateway dollars as a single `{ text }`, or null when the
 *  script printed no number (no token, no permission, no answer). The counter restarts at midnight. */
export function parseGatewayCost(stdout) {
  const cost = Number.parseFloat(stdout);
  return Number.isFinite(cost) ? [{ text: `$${cost.toFixed(2)}/day` }] : null;
}

// [Herdr's agent icon or a literal tag, tinted (one-color glyph) or drawn as is, key,
//  [command, args], parser].
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
  ["📓", true, "colab", ["sh", ["-c", COLAB_USAGE]], parseColabUsage],
  ["☁️", true, "gateway", ["sh", ["-c", GATEWAY_COST]], parseGatewayCost],
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
    const windowText = ({ label, used, resetsAt, text }) => {
      const remaining = used >= RESET_PERCENT ? formatResetRemaining(resetsAt) : null;
      const value = text ?? remaining ?? `${used}%`;
      // A countdown already means "until reset", so drop the window's own length (5h, 7d, ...)
      // and keep only a pool tag such as Antigravity's.
      const prefix = remaining ? label?.replace(/\d+[hd]$/, "") : label;
      return div()
        .text_color(used >= WARN_PERCENT ? t.blocked : t.foreground)
        .child(prefix ? `${prefix}${value}` : value);
    };
    // As agents-emoji draws an agent's icon from Herdr's program table.
    const icon = (path, tint) => {
      if (!path.endsWith(".svg")) {
        return div().text_color(t.dim).child(path);
      }
      return tint
        ? svg(path).size(ICON_SIZE).flex_none().text_color(t.foreground)
        : image(path).w(ICON_SIZE).h(ICON_SIZE).flex_none();
    };
    const provider = ([path, tint, name]) => {
      const windows = this.limits.get(name);
      return h_flex()
        .gap(2)
        .items_center()
        .child(icon(path, tint))
        .children(windows?.length ? windows.map(windowText) : [div().text_color(t.dim).child("—")]);
    };
    return h_flex()
      .size_full()
      .px(8)
      .gap(8)
      .items_center()
      .justify_end()
      .bg(t.chrome)
      .border_t(1)
      .border_color(t.border)
      .text_size(11.8)
      .children(PROVIDERS.map(provider));
  }
}
