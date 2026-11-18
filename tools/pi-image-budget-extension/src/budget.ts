// This TypeScript file is executed with Bun.

export const OMITTED = "[image omitted: only the newest images of this conversation are sent]";

interface Slot {
  readonly content: unknown[];
  readonly index: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isImage(block: unknown): boolean {
  return isRecord(block) && block["type"] === "image";
}

function contentOf(message: unknown): unknown[] {
  const content = isRecord(message) ? message["content"] : undefined;
  return Array.isArray(content) ? content : [];
}

// Oldest first, so everything before the newest kept slot is what gets dropped.
function slotsOf(message: unknown): Slot[] {
  const content = contentOf(message);
  const slots: Slot[] = [];
  content.forEach((block, index) => {
    if (isImage(block)) {
      slots.push({ content, index });
    }
  });
  return slots;
}

export interface ImageCaps {
  readonly keep: number;
  readonly perMessage: number;
}

/** The caps a model declares, or undefined when it declares neither. */
export function imageCapsOf(
  images:
    | { readonly maxPerMessage?: number | undefined; readonly maxPerRequest?: number | undefined }
    | undefined,
): ImageCaps | undefined {
  const keep = images?.maxPerRequest ?? Number.POSITIVE_INFINITY;
  const perMessage = images?.maxPerMessage ?? keep;
  if (!Number.isFinite(keep) && !Number.isFinite(perMessage)) {
    return undefined;
  }
  return { keep, perMessage };
}

/** Replaces images beyond the newest `keep` of the request, or beyond the newest `perMessage` of
 *  one message, with a text note, so the request stays under a provider's declared caps. Returns
 *  how many images were dropped. */
export function capImages(messages: readonly unknown[], keep: number, perMessage: number): number {
  const kept: Slot[] = [];
  const dropped: Slot[] = [];
  messages.forEach((message) => {
    const slots = slotsOf(message);
    const cut = Math.max(0, slots.length - perMessage);
    dropped.push(...slots.slice(0, cut));
    kept.push(...slots.slice(cut));
  });
  dropped.push(...kept.slice(0, Math.max(0, kept.length - keep)));
  dropped.forEach(({ content, index }) => {
    content[index] = { type: "text", text: OMITTED };
  });
  return dropped.length;
}
