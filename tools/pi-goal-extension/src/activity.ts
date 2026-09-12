// Runs with Bun. Dependency-free protocol shared by goal, loop and tmux extensions.
export interface ActivityBus {
  readonly emit: (channel: string, data: unknown) => void;
  readonly on: (
    channel: string,
    listener: (data: unknown) => void,
  ) => () => void;
}
export interface ActivitySnapshot {
  readonly source: "loop" | "tmux";
  readonly ownsContinuation: boolean;
  readonly pendingDelivery: boolean;
  readonly tasks: readonly string[];
}
export interface ActivityRequest {
  readonly sessionId: string;
  readonly respond: (snapshot: ActivitySnapshot) => void;
}
export interface TaskNotice {
  readonly sessionId: string;
  readonly name: string;
}

const QUERY = "pi:goal:activity-query:v1";
const LAUNCH = "pi:goal:task-launch:v1";

function isRequest(value: unknown): value is ActivityRequest {
  return (
    typeof value === "object" &&
    value !== null &&
    "sessionId" in value &&
    typeof value.sessionId === "string" &&
    "respond" in value &&
    typeof value.respond === "function"
  );
}

function isTaskNotice(value: unknown): value is TaskNotice {
  return (
    typeof value === "object" &&
    value !== null &&
    "sessionId" in value &&
    typeof value.sessionId === "string" &&
    "name" in value &&
    typeof value.name === "string"
  );
}

export function queryActivity(
  bus: ActivityBus,
  sessionId: string,
): readonly ActivitySnapshot[] {
  const snapshots: ActivitySnapshot[] = [];
  const request: ActivityRequest = {
    sessionId,
    respond: (snapshot) => {
      snapshots.push(snapshot);
    },
  };
  bus.emit(QUERY, request);
  return snapshots;
}

export function announceTask(bus: ActivityBus, notice: TaskNotice): void {
  bus.emit(LAUNCH, notice);
}

export function subscribeTasks(
  bus: ActivityBus,
  listener: (notice: TaskNotice) => void,
): () => void {
  return bus.on(LAUNCH, (value) => {
    if (isTaskNotice(value)) {
      listener(value);
    }
  });
}

export class ActivityProvider {
  readonly #bus: ActivityBus;
  readonly #snapshot: () => ActivitySnapshot;
  #remove: (() => void) | undefined;

  constructor(bus: ActivityBus, snapshot: () => ActivitySnapshot) {
    this.#bus = bus;
    this.#snapshot = snapshot;
  }

  start(sessionId: string): void {
    this.stop();
    this.#remove = this.#bus.on(QUERY, (request) => {
      if (isRequest(request) && request.sessionId === sessionId) {
        request.respond(this.#snapshot());
      }
    });
  }

  stop(): void {
    this.#remove?.();
    this.#remove = undefined;
  }
}
