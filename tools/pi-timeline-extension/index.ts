// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { StringEnum } from "@earendil-works/pi-ai";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { Type, type Static } from "typebox";
import { DurableTimelineStore } from "./src/store.ts";
import {
  addTask,
  EMPTY_TIMELINE,
  progress,
  renderTimeline,
  STATUSES,
  updateTask,
  type TaskPatch,
  type Timeline,
} from "./src/tree.ts";

const STATUS_KEY = "timeline";

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

interface Session {
  readonly release: (() => Promise<void>) | undefined;
  readonly store: DurableTimelineStore;
  timeline: Timeline;
}

function reportWith(context: ExtensionContext): (error: unknown) => void {
  return (error): void => {
    context.ui.notify(`Timeline storage stopped: ${String(error)}`, "error");
  };
}

async function openStore(context: ExtensionContext): Promise<Omit<Session, "timeline">> {
  const file: string | undefined = context.sessionManager.getSessionFile();
  const report = reportWith(context);
  if (file === undefined) {
    const store = await DurableTimelineStore.open(new MemoryStorage(), report);
    return { release: undefined, store };
  }
  // The sidecar stays with the session file, so a resumed session finds its own tree.
  const directory = `${file}.timeline-durable`;
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const release = await lock(directory, { retries: 0 });
  const storage = await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true });
  return { release, store: await DurableTimelineStore.open(storage, report) };
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

function show(context: ExtensionContext, timeline: Timeline): void {
  const percent = Math.round(progress(timeline) * 100);
  context.ui.setStatus(STATUS_KEY, `${String(percent)}%`);
}

export default function timelineExtension(pi: ExtensionAPI): void {
  let session: Session | undefined = undefined;

  const close = async (): Promise<void> => {
    const closing = session;
    session = undefined;
    try {
      await closing?.store.close();
    } finally {
      await closing?.release?.();
    }
  };

  const ensure = async (context: ExtensionContext): Promise<Session> => {
    if (session !== undefined) {
      return session;
    }
    const opened = await openStore(context);
    const stored: Timeline | undefined = await opened.store.restore();
    session = { ...opened, timeline: stored ?? EMPTY_TIMELINE };
    return session;
  };

  pi.registerTool({
    name: "timeline",
    label: "Timeline",
    description:
      "Record this session's task tree and its progress: add a task or subtask, change a task's status, or read the tree.",
    promptSnippet: "Record the task tree and how far each task has got",
    promptGuidelines: [
      'Use timeline with action "add" to break work into tasks and subtasks before starting it, and action "update" to mark each one active, done or blocked as you go.',
      'Use timeline with action "list" when you need the current plan, its ids and its progress.',
    ],
    parameters,
    async execute(_toolCallId, params, _signal, _onUpdate, context) {
      const active = await ensure(context);
      active.timeline = apply(active, params);
      active.store.save(active.timeline);
      show(context, active.timeline);
      return {
        content: [{ type: "text", text: renderTimeline(active.timeline).join("\n") }],
        details: { progress: progress(active.timeline), tasks: active.timeline.tasks.length },
      };
    },
  });

  pi.registerCommand("timeline", {
    description: "Show this session's task tree and its progress",
    handler: async (_args, context) => {
      const active = await ensure(context);
      show(context, active.timeline);
      context.ui.notify(renderTimeline(active.timeline).join("\n"), "info");
    },
  });

  pi.on("session_start", async (_event, context) => {
    await close();
    const active = await ensure(context);
    show(context, active.timeline);
  });
  pi.on("session_shutdown", async () => {
    await close();
  });
}
