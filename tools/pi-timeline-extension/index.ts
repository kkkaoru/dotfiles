// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { StringEnum } from "@earendil-works/pi-ai";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { ExtensionAPI, ExtensionContext, ThemeColor } from "@earendil-works/pi-coding-agent";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { Type, type Static } from "typebox";
import { ageOf, checkpoint, everyTick, NUDGE_TYPE } from "./src/keepalive.ts";
import { DurableTimelineStore, lastWrite } from "./src/store.ts";
import { plamoDisplayFromSettings } from "./src/live.ts";
import type { TranslatedTimeline } from "./src/translate.ts";
import {
  addTask,
  EMPTY_TIMELINE,
  progress,
  renderTimeline,
  STATUSES,
  updateTask,
  type Colour,
  type Role,
  type TaskPatch,
  type Timeline,
} from "./src/tree.ts";

const STATUS_KEY = "timeline";
const WIDGET_KEY = "timeline";
/** Dracula, through pi's own theme: green finished, purple running, red blocked, comment for the rest. */
const TOKEN: Readonly<Record<Role, ThemeColor>> = {
  active: "accent",
  blocked: "error",
  done: "success",
  empty: "dim",
  muted: "muted",
  pending: "muted",
  total: "accent",
};

/** The widget's colours come from the active pi theme, so a switch repaints in the new palette. */
function colourOf(context: ExtensionContext): Colour {
  return (role, text) => context.ui.theme.fg(TOKEN[role], text);
}

const statusSchema = Type.Union(STATUSES.map((status) => Type.Literal(status)));
const parameters = Type.Object({
  action: StringEnum(["add", "update", "list"] as const),
  id: Type.Optional(Type.String({ description: "Task id, as shown by list" })),
  note: Type.Optional(Type.String({ description: "Short note: the result or the blocker" })),
  parent: Type.Optional(Type.String({ description: "Parent task id, to make this a subtask" })),
  status: Type.Optional(statusSchema),
  title: Type.Optional(Type.String({ description: "What the task is" })),
  weight: Type.Optional(
    Type.Integer({ description: "Relative size among its siblings, default 1", minimum: 1 }),
  ),
});
type Params = Static<typeof parameters>;

/** A store and when its sidecar last changed, which is what the head's age is measured from. */
interface Opened {
  readonly release: (() => Promise<void>) | undefined;
  readonly store: DurableTimelineStore;
  readonly updatedAt: number | undefined;
}

interface Session extends Opened {
  /** The tree as last painted, so a repaint of the age alone need not translate it again. */
  shown: Timeline;
  /** Tool calls since the last timeline call, which is how a stale tree is noticed mid-turn. */
  steps: number;
  timeline: Timeline;
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

function patchOf(params: Params): TaskPatch {
  return {
    ...(params.note === undefined ? {} : { note: params.note }),
    ...(params.status === undefined ? {} : { status: params.status }),
    ...(params.title === undefined ? {} : { title: params.title }),
    ...(params.weight === undefined ? {} : { weight: params.weight }),
  };
}

function added(timeline: Timeline, params: Params): Timeline {
  if (params.title === undefined) {
    throw new Error("action add needs a title.");
  }
  return addTask(timeline, {
    parent: params.parent ?? null,
    title: params.title,
    ...(params.weight === undefined ? {} : { weight: params.weight }),
  }).timeline;
}

function updated(timeline: Timeline, params: Params): Timeline {
  if (params.id === undefined) {
    throw new Error("action update needs an id.");
  }
  return updateTask(timeline, params.id, patchOf(params));
}

function apply(active: Session, params: Params): Timeline {
  switch (params.action) {
    case "add": {
      return added(active.timeline, params);
    }
    case "update": {
      return updated(active.timeline, params);
    }
    default: {
      return active.timeline;
    }
  }
}

/** A tree whose every task is done has nothing left to track, so the editor gets its rows back. */
function shows(timeline: Timeline): boolean {
  return timeline.tasks.some((task) => task.status !== "done");
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
    const stored: Timeline | undefined = await opened.store.restore();
    const timeline: Timeline = stored ?? EMPTY_TIMELINE;
    return {
      ...opened,
      shown: timeline,
      steps: 0,
      timeline,
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
    const { timeline } = active;
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
    const percent = Math.round(progress(active.timeline) * 100);
    context.ui.setStatus(STATUS_KEY, `${String(percent)}%`);
    active.shown = active.timeline;
    // English first, so a slow translation never leaves the tree blank.
    paint(
      context,
      shows(active.timeline)
        ? renderTimeline(active.timeline, colourOf(context), ageOf(active))
        : undefined,
    );
    localise(context, active);
  };

  /** The age moves even while nothing changes, so the head is repainted on its own. */
  const repaint = (context: ExtensionContext, active: Session): void => {
    if (shows(active.timeline)) {
      paint(context, renderTimeline(active.shown, colourOf(context), ageOf(active)));
    }
  };

  return { localise, repaint, show };
}

export default function timelineExtension(pi: ExtensionAPI, options: Options = {}): void {
  const display = "display" in options ? options.display : plamoDisplayFromSettings();
  const store = storeSession();
  const { repaint, show } = painter(display);

  pi.registerTool({
    name: "timeline",
    label: "Timeline",
    description:
      "Record this session's task tree and its progress: add a task or subtask, change a task's status, or read the tree.",
    promptSnippet: "Plan multi-step work as a task tree, and keep it current as the work moves",
    promptGuidelines: [
      'Before substantive multi-step work, reuse this goal\'s tree or break the work into tasks and checkable subtasks with timeline action "add". Group work by deliverable.',
      "At meaningful checkpoints, mark what finished as done, keep the current task active with a short note of its result, and add tasks when the scope changes. Preserve completed work.",
      "Verify a task before marking it done; a blocked task keeps a note saying what blocks it, and a dependency is rechecked once before the blocker is recorded again.",
      'Before a final answer on planned work, save progress and use timeline action "list" to confirm what remains; use it also when you need the plan\'s ids.',
    ],
    parameters,
    async execute(_toolCallId, params, _signal, _onUpdate, context) {
      const active = await store.ensure(context);
      const next: Timeline = apply(active, params);
      // The call is the checkpoint itself, and only a real change moves the tree's age.
      active.steps = 0;
      if (next !== active.timeline) {
        active.timeline = next;
        active.updatedAt = Date.now();
      }
      active.store.save(active.timeline);
      show(context, active);
      return {
        content: [{ type: "text", text: renderTimeline(active.timeline).join("\n") }],
        details: { progress: progress(active.timeline), tasks: active.timeline.tasks.length },
      };
    },
  });

  pi.registerCommand("timeline", {
    description: "Show this session's task tree and its progress",
    handler: async (_args, context) => {
      const active = await store.ensure(context);
      show(context, active);
      context.ui.notify(renderTimeline(active.timeline).join("\n"), "info");
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
    const notice = checkpoint(active);
    if (notice !== undefined) {
      pi.sendMessage(
        { content: notice, customType: NUDGE_TYPE, display: false },
        { deliverAs: "steer" },
      );
    }
  });
}
