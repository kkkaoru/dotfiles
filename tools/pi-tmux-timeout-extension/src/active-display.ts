// This TypeScript file is executed with Bun.
import type { CompletionDeliveryContext } from "./delivery.ts";
import { formatLocalTimestamp } from "./policy.ts";
import { DEFAULT_ESTIMATED_DURATION_SECONDS, type TmuxLaunch } from "./tmux.ts";

export const ACTIVE_DISPLAY_ENTRY_TYPE = "pi-tmux-active-display-v1";
const MAX_TASK_IDENTITY_CHARACTERS = 160;

export interface ActiveTaskDisplayState {
  readonly dismissedSessionNames: readonly string[];
  readonly hidden: boolean;
}

function taskIdentity(command: string): string {
  return command.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_TASK_IDENTITY_CHARACTERS);
}

function runningTaskName(launch: TmuxLaunch): string {
  const submittedDate = new Date(launch.submittedAt);
  const estimatedCompletionDate = new Date(
    launch.estimatedCompletionAt ??
      submittedDate.getTime() + DEFAULT_ESTIMATED_DURATION_SECONDS * 1000,
  );
  const submittedAt: string = formatLocalTimestamp(submittedDate, "submitted");
  const estimatedCompletionAt: string = formatLocalTimestamp(estimatedCompletionDate, "submitted");
  return `⏳ ${submittedAt} → ${estimatedCompletionAt} ${taskIdentity(launch.taskCommand)}`;
}

function customDisplayData(entry: unknown): unknown {
  if (typeof entry !== "object" || entry === null) {
    return undefined;
  }
  if (!("type" in entry) || entry.type !== "custom") {
    return undefined;
  }
  if (!("customType" in entry) || entry.customType !== ACTIVE_DISPLAY_ENTRY_TYPE) {
    return undefined;
  }
  return "data" in entry ? entry.data : undefined;
}

function stringArray(value: unknown): readonly string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const strings: string[] = value.filter(
    (item: unknown): item is string => typeof item === "string",
  );
  return strings.length === value.length ? strings : undefined;
}

function displayState(entry: unknown): ActiveTaskDisplayState | undefined {
  const data: unknown = customDisplayData(entry);
  if (typeof data !== "object" || data === null || !("hidden" in data)) {
    return undefined;
  }
  if (typeof data.hidden !== "boolean" || !("dismissedSessionNames" in data)) {
    return undefined;
  }
  const dismissedSessionNames: readonly string[] | undefined = stringArray(
    data.dismissedSessionNames,
  );
  return dismissedSessionNames === undefined
    ? undefined
    : { dismissedSessionNames, hidden: data.hidden };
}

export function recoverActiveTaskDisplayState(entries: readonly unknown[]): ActiveTaskDisplayState {
  return (
    entries
      .flatMap((entry: unknown): readonly ActiveTaskDisplayState[] => {
        const state: ActiveTaskDisplayState | undefined = displayState(entry);
        return state === undefined ? [] : [state];
      })
      .at(-1) ?? { dismissedSessionNames: [], hidden: false }
  );
}

export class ActiveTaskDisplay {
  #context: CompletionDeliveryContext | undefined;
  readonly #dismissedSessionNames = new Set<string>();
  #hidden = false;
  #launches: readonly TmuxLaunch[] = [];

  setContext(context: CompletionDeliveryContext): void {
    this.#context = context;
    this.#render();
  }

  update(launches: readonly TmuxLaunch[]): void {
    this.#launches = [...launches];
    this.#render();
  }

  restore(state: ActiveTaskDisplayState): void {
    this.#dismissedSessionNames.clear();
    state.dismissedSessionNames.map((sessionName: string): Set<string> =>
      this.#dismissedSessionNames.add(sessionName),
    );
    this.#hidden = state.hidden;
    this.#render();
  }

  dismissActive(): number {
    const visible: readonly TmuxLaunch[] = this.#visibleLaunches();
    visible.map((launch: TmuxLaunch): Set<string> =>
      this.#dismissedSessionNames.add(launch.sessionName),
    );
    this.#render();
    return visible.length;
  }

  setHidden(hidden: boolean): void {
    this.#hidden = hidden;
    this.#render();
  }

  reset(): void {
    this.#dismissedSessionNames.clear();
    this.#hidden = false;
    this.#render();
  }

  state(): ActiveTaskDisplayState {
    return {
      dismissedSessionNames: [...this.#dismissedSessionNames],
      hidden: this.#hidden,
    };
  }

  activeCount(): number {
    return this.#launches.length;
  }

  visibleCount(): number {
    return this.#visibleLaunches().length;
  }

  clear(): void {
    this.#launches = [];
    this.#render();
    this.#context = undefined;
  }

  #visibleLaunches(): readonly TmuxLaunch[] {
    return this.#hidden
      ? []
      : this.#launches.filter(
          (launch: TmuxLaunch): boolean => !this.#dismissedSessionNames.has(launch.sessionName),
        );
  }

  #render(): void {
    const visible: readonly TmuxLaunch[] = this.#visibleLaunches();
    const count: number = visible.length;
    this.#context?.ui.setStatus("tmux-running", count === 0 ? undefined : `tmux:${String(count)}`);
    this.#context?.ui.setWidget?.(
      "tmux-running-tasks",
      count === 0
        ? undefined
        : visible.map((launch: TmuxLaunch): string => runningTaskName(launch)),
    );
  }
}
