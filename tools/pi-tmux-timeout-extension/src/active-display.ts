// This TypeScript file is executed with Bun.
import type { CompletionDeliveryContext } from "./delivery.ts";
import { formatLocalTimestamp } from "./policy.ts";
import { estimatedCompletionTime, type TmuxLaunch } from "./tmux.ts";

export const ACTIVE_DISPLAY_ENTRY_TYPE = "pi-tmux-active-display-v1";
const MAX_TASK_IDENTITY_CHARACTERS = 160;

export interface ActiveTaskDisplayState {
  readonly dismissedSessionNames: readonly string[];
  readonly hidden: boolean;
  readonly acknowledgedAt?: Readonly<Record<string, number>>;
}

function taskIdentity(command: string): string {
  return command.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_TASK_IDENTITY_CHARACTERS);
}

function overdueIndicator(acknowledged: boolean): string {
  return acknowledged ? "✓ checked · running" : "⚠ overdue";
}

function runningTaskName(launch: TmuxLaunch, acknowledged: boolean): string {
  const submittedDate = new Date(launch.submittedAt);
  const estimatedCompletionDate = new Date(estimatedCompletionTime(launch));
  const indicator: string =
    estimatedCompletionDate.getTime() > Date.now() ? "⏳" : overdueIndicator(acknowledged);
  const submittedAt: string = formatLocalTimestamp(submittedDate, "submitted");
  const estimatedCompletionAt: string = formatLocalTimestamp(estimatedCompletionDate, "submitted");
  return `${indicator} ${submittedAt} → ${estimatedCompletionAt} ${taskIdentity(launch.taskCommand)}`;
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

function acknowledgedState(data: unknown): Readonly<Record<string, number>> | null | undefined {
  if (data === undefined) {
    return undefined;
  }
  if (typeof data !== "object" || data === null || Array.isArray(data)) {
    return null;
  }
  if (
    !Object.values(data).every(
      (time: unknown): boolean =>
        typeof time === "number" && Number.isSafeInteger(time) && time >= 0,
    )
  ) {
    return null;
  }
  return Object.fromEntries(Object.entries(data));
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
  const acknowledged = acknowledgedState(
    "acknowledgedAt" in data ? data.acknowledgedAt : undefined,
  );
  if (dismissedSessionNames === undefined || acknowledged === null) {
    return undefined;
  }
  return acknowledged === undefined
    ? { dismissedSessionNames, hidden: data.hidden }
    : { dismissedSessionNames, hidden: data.hidden, acknowledgedAt: acknowledged };
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
  readonly #acknowledgedAt = new Map<string, number>();
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
    this.#acknowledgedAt.clear();
    Object.entries(state.acknowledgedAt ?? {}).map(([name, time]): Map<string, number> =>
      this.#acknowledgedAt.set(name, time),
    );
    this.#render();
  }

  acknowledge(launches: readonly TmuxLaunch[], now: number): readonly TmuxLaunch[] {
    const active = new Set(this.#launches.map((launch: TmuxLaunch): string => launch.sessionName));
    const matched = launches.filter(
      (launch: TmuxLaunch): boolean =>
        active.has(launch.sessionName) && estimatedCompletionTime(launch) <= now,
    );
    matched.map((launch: TmuxLaunch): Map<string, number> =>
      this.#acknowledgedAt.set(launch.sessionName, now),
    );
    if (matched.length > 0) {
      this.#render();
    }
    return matched;
  }

  remind(launches: readonly TmuxLaunch[]): boolean {
    const changed = launches
      .map((launch: TmuxLaunch): boolean => this.#acknowledgedAt.delete(launch.sessionName))
      .includes(true);
    if (changed) {
      this.#render();
    }
    return changed;
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
    const acknowledgedAt = Object.fromEntries(
      this.#launches
        .filter((launch: TmuxLaunch): boolean => this.#acknowledgedAt.has(launch.sessionName))
        .map((launch: TmuxLaunch): readonly [string, number] => [
          launch.sessionName,
          this.#acknowledgedAt.get(launch.sessionName) ?? 0,
        ]),
    );
    return {
      dismissedSessionNames: [...this.#dismissedSessionNames],
      hidden: this.#hidden,
      ...(Object.keys(acknowledgedAt).length === 0 ? {} : { acknowledgedAt }),
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
        : visible.map((launch: TmuxLaunch): string =>
            runningTaskName(launch, this.#acknowledgedAt.has(launch.sessionName)),
          ),
    );
  }
}
