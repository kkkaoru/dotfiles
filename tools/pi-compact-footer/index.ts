// This TypeScript file is executed with Bun.
import { homedir } from "node:os";
import path from "node:path";
import type {
  ExtensionAPI,
  ExtensionContext,
  ReadonlyFooterDataProvider,
  Theme,
} from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

/** Extension statuses that keep a place on the row, in this order. The rest are dropped: pi's own
 *  footer prints every status on one line and cuts it off mid-word once they no longer fit. */
export const SHOWN: readonly string[] = [
  "gateway-cost",
  "pi-effort-thinking",
  "pi-effort-dynamic",
  "plamo-translate",
  "strata",
  "ponytail",
];

/** `● 🐴 ponytail: ⚡ FULL` -> `🐴 FULL`: the mode is worth a word, not a sentence.
 *  shortcut: reads ponytail's own label, so a changed format just shows its text again. */
export function shortStatus(key: string, text: string): string {
  if (key !== "ponytail") {
    return text;
  }
  const label: string | undefined = /ponytail:\s*(.+)$/u.exec(text)?.[1];
  return label === undefined ? text : `🐴 ${label}`;
}

/** The folder names the checkout; the window title already shows the whole path. */
export function folderOf(cwd: string): string {
  return cwd.startsWith(homedir()) ? path.basename(cwd) : cwd;
}

export function shortModel(id: string | undefined): string {
  return id?.split("/").pop() ?? "";
}

/** `/session <8 hex chars>` resumes this session, so eight characters are enough to show. */
export function shortSession(id: string): string {
  return id.slice(0, 8);
}

function windowText(window: number): string {
  return window < 1000 ? String(window) : `${String(Math.round(window / 1000))}k`;
}

/** `30.2%/262k`, or the window alone while no response has measured the context yet. */
export function contextText(
  usage: { readonly percent: number | null; readonly contextWindow: number } | undefined,
): string {
  if (usage === undefined) {
    return "";
  }
  const window = windowText(usage.contextWindow);
  return usage.percent === null ? window : `${usage.percent.toFixed(1)}%/${window}`;
}

/** Left text padded out to the right text, both cut down rather than wrapped. */
export function twoEnds(left: string, right: string, width: number): string {
  const room = width - visibleWidth(left);
  if (room < 2) {
    return truncateToWidth(left, width, "");
  }
  const tail = visibleWidth(right) <= room - 2 ? right : truncateToWidth(right, room - 2, "");
  return left + " ".repeat(room - visibleWidth(tail)) + tail;
}

export function footerLines(
  ctx: ExtensionContext,
  footerData: ReadonlyFooterDataProvider,
  theme: Theme,
  width: number,
): string[] {
  const branch = footerData.getGitBranch();
  const folder = folderOf(ctx.cwd);
  const statuses = footerData.getExtensionStatuses();
  const stats = [
    contextText(ctx.getContextUsage()),
    ...SHOWN.flatMap((key) => {
      const text = statuses.get(key);
      return text === undefined ? [] : [shortStatus(key, text)];
    }),
  ]
    .filter((part) => part.length > 0)
    .join(" · ");
  return [
    twoEnds(
      theme.fg("text", branch === null ? folder : `${folder} (${branch})`),
      theme.fg("dim", shortModel(ctx.model?.id)),
      width,
    ),
    twoEnds(
      theme.fg("dim", stats),
      theme.fg("dim", shortSession(ctx.sessionManager.getSessionId())),
      width,
    ),
  ];
}

export default function compactFooter(pi: ExtensionAPI): void {
  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui") {
      return;
    }
    ctx.ui.setFooter((tui, theme, footerData) => {
      const dispose = footerData.onBranchChange(() => {
        tui.requestRender();
      });
      return {
        dispose,
        invalidate() {},
        render: (width: number) => footerLines(ctx, footerData, theme, width),
      };
    });
  });
}
