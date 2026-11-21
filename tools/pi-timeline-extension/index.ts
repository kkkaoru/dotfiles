// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { ExtensionAPI, ExtensionContext, ThemeColor } from "@earendil-works/pi-coding-agent";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { ageOf, checkpoint, everyTick, NUDGE_TYPE } from "./src/keepalive.ts";
import { DurableTimelineStore, lastWrite } from "./src/store.ts";
import { plamoDisplayFromSettings } from "./src/live.ts";
import type { TranslatedTimeline } from "./src/translate.ts";
import { renderTimeline, type Colour, type Role } from "./src/render.ts";
import { apply, boardText, parameters, shows } from "./src/tool.ts";
import { activeTree, EMPTY_BOARD, progress, type Board, type Timeline } from "./src/tree.ts";

const STATUS_KEY = "timeline";
const WIDGET_KEY = "timeline";
/** Dracula, through pi's own theme: green finished, purple running, red blocked, comment for the rest. */
const TOKEN: Readonly<Record<Role, ThemeColor>> = {
  active: "accent",
  blocked: "error",
  done: "success",
  empty: "dim",
  muted: "muted",
  paused: "muted",
  pending: "muted",
  retired: "dim",
  total: "accent",
};

/** The widget's colours come from the active pi theme, so a switch repaints in the new palette. */
function colourOf(context: ExtensionContext): Colour {
  return (role, text) => context.ui.theme.fg(TOKEN[role], text);
}

/** A store and when its sidecar last changed, which is what the head's age is measured from. */
interface Opened {
  readonly release: (() => Promise<void>) | undefined;
  readonly store: DurableTimelineStore;
  readonly updatedAt: number | undefined;
}

interface Session extends Opened {
  /** Every plan of this session, and which one the widget shows. */
  board: Board;
  /** The active tree as last painted, so a repaint of the age alone need not translate it again. */
  shown: Timeline;
  /** Tool calls since the last timeline call, which is how a stale tree is noticed mid-turn. */
  steps: number;
  updatedAt: number;
}

interface Holder {
  readonly release: () => Promise<void>;
  readonly store: DurableTimelineStore;
  refs: number;
}

/**
 * The open sidecars live on globalThis: /reload evaluates this module again while the previous copy
 * still holds the lock, so a second lock would fail and the tree would stop being shown.
 */
const HELD_KEY = Symbol.for("dotfiles.timeline.held");

function held(): Map<string, Holder> {
  const shared = globalThis as typeof globalThis & { [HELD_KEY]?: Map<string, Holder> };
  shared[HELD_KEY] ??= new Map<string, Holder>();
  return shared[HELD_KEY];
}

async function releaseHeld(entry: Holder, directory: string): Promise<void> {
  entry.refs -= 1;
  if (entry.refs > 0) {
    return;
  }
  held().delete(directory);
  try {
    await entry.store.close();
  } finally {
    await entry.release();
  }
}

async function reuse(directory: string, entry: Holder): Promise<Opened> {
  entry.refs += 1;
  return {
    release: async () => releaseHeld(entry, directory),
    store: entry.store,
    updatedAt: await lastWrite(directory),
  };
}

function reportWith(context: ExtensionContext): (error: unknown) => void {
  return (error): void => {
    context.ui.notify(`Timeline storage stopped: ${String(error)}`, "error");
  };
}

/** Open the sidecar this session keeps its tree in, and hold its lock for as long as it is used. */
async function openSidecar(directory: string, report: (error: unknown) => void): Promise<Opened> {
  let release: (() => Promise<void>) | undefined = undefined;
  try {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    release = await lock(directory, { retries: 0 });
    const storage = await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true });
    const store = await DurableTimelineStore.open(storage, report);
    const entry: Holder = { refs: 1, release, store };
    held().set(directory, entry);
    return {
      release: async () => releaseHeld(entry, directory),
      store,
      updatedAt: await lastWrite(directory),
    };
  } catch (error) {
    // Release a lock we took so a later session can open the sidecar.
    await release?.();
    throw error;
  }
}

async function openStore(context: ExtensionContext): Promise<Opened> {
  const file: string | undefined = context.sessionManager.getSessionFile();
  const report = reportWith(context);
  const inMemory = async (): Promise<Opened> => {
    const store = await DurableTimelineStore.open(new MemoryStorage(), report);
    return { release: async () => store.close(), store, updatedAt: undefined };
  };
  if (file === undefined) {
    return inMemory();
  }
  // The sidecar stays with the session file, so a resumed session finds its own tree.
  const directory = `${file}.timeline-durable`;
  const shared = held().get(directory);
  if (shared !== undefined) {
    return reuse(directory, shared);
  }
  try {
    return await openSidecar(directory, report);
  } catch (error) {
    // Another pi holds this session's sidecar, or it cannot be opened: a tree in memory beats none.
    context.ui.notify(
      `Timeline: this session's tree stays in memory (${String(error)}).`,
      "warning",
    );
    return inMemory();
  }
}

function paint(context: ExtensionContext, lines?: string[]): void {
  // The footer row is shared and need not carry this status, so the tree keeps a place of its own.
  context.ui.setWidget(WIDGET_KEY, lines, { placement: "belowEditor" });
}

interface Store {
  readonly close: () => Promise<void>;
  readonly ensure: (context: ExtensionContext) => Promise<Session>;
}

function storeSession(): Store {
  let session: Session | undefined = undefined;
  let opening: Promise<Session> | undefined = undefined;

  const close = async (): Promise<void> => {
    const closing = session;
    session = undefined;
    await closing?.release?.();
  };

  const open = async (context: ExtensionContext): Promise<Session> => {
    const opened = await openStore(context);
    const stored: Board | undefined = await opened.store.restore();
    const board: Board = stored ?? EMPTY_BOARD;
    return {
      ...opened,
      board,
      shown: activeTree(board),
      steps: 0,
      updatedAt: opened.updatedAt ?? Date.now(),
    };
  };

  const ensure = async (context: ExtensionContext): Promise<Session> => {
    if (session !== undefined) {
      return session;
    }
    // Two tool calls can arrive before the store is open; one open serves both.
    opening ??= open(context).finally(() => {
      opening = undefined;
    });
    session = await opening;
    return session;
  };

  return { close, ensure };
}

export interface Options {
  /** Injected in tests; the real one talks to the PLaMo server that pi-plamo-translate runs. */
  readonly display?: TranslatedTimeline | undefined;
}

type Draw = (context: ExtensionContext, active: Session) => void;
type Painter = Record<"localise" | "repaint" | "show", Draw>;

/** Everything that paints the tree, so the extension itself stays a wiring of handlers. */
function painter(display: TranslatedTimeline | undefined): Painter {
  let generation = 0;

  const localise = (context: ExtensionContext, active: Session): void => {
    const timeline = activeTree(active.board);
    if (display === undefined || !shows(timeline)) {
      return;
    }
    const mine = (generation += 1);
    display
      .translate(timeline)
      .then((translated) => {
        // A newer tree is already drawn, and its own translation is on the way.
        if (mine === generation) {
          active.shown = translated;
          paint(context, renderTimeline(translated, colourOf(context), ageOf(active)));
        }
      })
      .catch(() => {
        // Translate() answers the English itself, so there is nothing left to report here.
      });
  };

  const show = (context: ExtensionContext, active: Session): void => {
    const timeline = activeTree(active.board);
    const percent = Math.round(progress(timeline) * 100);
    context.ui.setStatus(STATUS_KEY, `${String(percent)}%`);
    active.shown = timeline;
    // English first, so a slow translation never leaves the tree blank.
    paint(
      context,
      shows(timeline) ? renderTimeline(timeline, colourOf(context), ageOf(active)) : undefined,
    );
    localise(context, active);
  };

  /** The age moves even while nothing changes, so the head is repainted on its own. */
  const repaint = (context: ExtensionContext, active: Session): void => {
    if (shows(activeTree(active.board))) {
      paint(context, renderTimeline(active.shown, colourOf(context), ageOf(active)));
    }
  };

  return { localise, repaint, show };
}

/** The tool itself: one call records a change, saves it, and answers with the rendered tree. */
function registerTimelineTool(
  pi: ExtensionAPI,
  store: Store,
  show: (context: ExtensionContext, active: Session) => void,
): void {
  pi.registerTool({
    name: "timeline",
    label: "Timeline",
    description:
      "Record this session's plan and its progress: start a tree for a goal, add a task or subtask, change a task's status, priority, done units, estimate or note, or read the tree.",
    promptSnippet: "Plan multi-step work as a task tree, and keep it current as the work moves",
    promptGuidelines: [
      'Before substantive multi-step work, reuse this goal\'s tree or break the work into tasks and checkable subtasks with timeline action "add". Group work by deliverable, and give a new goal its own tree with action "start".',
      "At meaningful checkpoints, mark what finished as done, keep the current task active with a short note of its result, and add tasks when the scope changes. Preserve completed work.",
      "Verify a task before marking it done; a blocked task keeps a note saying what blocks it, and a dependency is rechecked once before the blocker is recorded again.",
      "Say how big the work is when it is uneven: set a weight relative to its siblings, record a remaining estimate in minutes when you know one, and mark partly finished work with done units (e.g. done [2, 5]). Use P0–P3 when the order matters. A task that leaves the goal is retired, not deleted, and work waiting on someone else is paused or blocked.",
      'Before a final answer on planned work, save progress and use timeline action "list" to confirm what remains; use it also when you need the plan\'s ids.',
    ],
    parameters,
    async execute(_toolCallId, params, _signal, _onUpdate, context) {
      const active = await store.ensure(context);
      const next: Board = apply(active.board, params);
      // The call is the checkpoint itself, and only a real change moves the tree's age.
      active.steps = 0;
      if (next !== active.board) {
        active.board = next;
        active.updatedAt = Date.now();
      }
      active.store.save(active.board);
      show(context, active);
      const timeline = activeTree(active.board);
      return {
        content: [{ type: "text", text: boardText(active.board) }],
        details: {
          progress: progress(timeline),
          tasks: timeline.tasks.length,
          tree: active.board.active,
        },
      };
    },
  });
}

export default function timelineExtension(pi: ExtensionAPI, options: Options = {}): void {
  const display = "display" in options ? options.display : plamoDisplayFromSettings();
  const store = storeSession();
  const { repaint, show } = painter(display);

  registerTimelineTool(pi, store, show);

  pi.registerCommand("timeline", {
    description: "Show this session's task tree and its progress",
    handler: async (_args, context) => {
      const active = await store.ensure(context);
      show(context, active);
      context.ui.notify(boardText(active.board), "info");
    },
  });

  const ticker: { stop?: () => void } = {};
  const stopTicker = (): void => {
    ticker.stop?.();
    delete ticker.stop;
  };

  pi.on("session_start", async (_event, context) => {
    stopTicker();
    await store.close();
    const active = await store.ensure(context);
    show(context, active);
    ticker.stop = everyTick(() => {
      repaint(context, active);
    });
  });
  pi.on("session_shutdown", async (_event, context) => {
    stopTicker();
    await store.close();
    paint(context);
  });
  // A turn can run for an hour, so a stale tree is reported mid-turn instead of at its end.
  pi.on("tool_call", async (event, context) => {
    if (event.toolName === "timeline") {
      return;
    }
    const active = await store.ensure(context);
    active.steps += 1;
    const notice = checkpoint(active, activeTree(active.board));
    if (notice !== undefined) {
      pi.sendMessage(
        { content: notice, customType: NUDGE_TYPE, display: false },
        { deliverAs: "steer" },
      );
    }
  });
}
