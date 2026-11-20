// This TypeScript file is executed with Bun.
import type {
  ExtensionAPI,
  ExtensionContext,
  ReadonlyFooterDataProvider,
  Theme,
} from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

/** Statuses that keep a place on the row, in this order. The effort level, the gateway price and
 *  the ponytail mode are not among them: the row is shared, and they are read elsewhere. */
export const SHOWN: readonly string[] = ["plamo-translate", "strata"];

/** `workers-ai/@cf/deepseek-ai/deepseek-v4.1-flash-1017` -> `workers-ai/deepseek-v4.1-flash-1017`:
 *  the provider decides routing and cost, the model path before it does not. */
export function modelText(provider: string | undefined, id: string | undefined): string {
  const model = id?.split("/").pop() ?? "";
  return provider === undefined || model === "" ? model : `${provider}/${model}`;
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
  const statuses = footerData.getExtensionStatuses();
  const parts = [
    contextText(ctx.getContextUsage()),
    ...SHOWN.flatMap((key) => {
      const text = statuses.get(key);
      return text === undefined ? [] : [text];
    }),
  ].filter((part) => part.length > 0);
  // Two keys can name the same thing; the row is shared, so each reading appears once.
  const stats = [...new Set(parts)].join(" · ");
  return [
    twoEnds("", theme.fg("dim", modelText(ctx.model?.provider, ctx.model?.id)), width),
    twoEnds(theme.fg("dim", stats), theme.fg("dim", ctx.sessionManager.getSessionId()), width),
  ];
}

export default function compactFooter(pi: ExtensionAPI): void {
  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui") {
      return;
    }
    ctx.ui.setFooter((_tui, theme, footerData) => ({
      dispose() {},
      invalidate() {},
      render: (width: number) => footerLines(ctx, footerData, theme, width),
    }));
  });
}
