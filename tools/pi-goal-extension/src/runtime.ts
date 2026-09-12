// Runs with Bun.
import { randomUUID } from "node:crypto";
import {
  type ActivityBus,
  queryActivity,
  subscribeTasks,
  type TaskNotice,
} from "./activity.ts";
import {
  accountUsage,
  createGoal,
  GOAL_ENTRY,
  type GoalState,
  pauseGoal,
  restoreGoal,
  resumeGoal,
  updateGoal,
} from "./state.ts";

export interface GoalHost {
  readonly bus: ActivityBus;
  readonly sessionId: string;
  isReady(): boolean;
  send(text: string): void;
  persist(type: string, data: unknown): void;
  display(goal: GoalState | null): void;
}
export interface StartInput {
  readonly objective: string;
  readonly tokenBudget: number | null;
}
export interface WaitInput {
  readonly delaySeconds: number;
  readonly reason: string;
}
interface Ticket {
  readonly text: string;
  readonly revision: number;
  readonly createdAt: number;
}

export const GOAL_MESSAGE_PREFIX: string = "[pi-goal-continuation:";
const POLL_MS: number = 5000;
const ACCEPT_TIMEOUT_MS: number = 30000;
const SECOND_MS: number = 1000;
const MIN_WAIT_SECONDS: number = 60;
const MAX_WAIT_SECONDS: number = 3600;
const BUSY_ERROR: string = "Agent is already processing a prompt";
const MAX_STALLED_TURNS: number = 3;
const CONTROL_TOOLS: ReadonlySet<string> = new Set([
  "get_goal",
  "update_goal",
  "goal_wait",
  "loop_wakeup",
  "loop_complete",
]);
const CONTINUATION: string =
  "Continue the explicitly established goal. Use current artifacts and external evidence. Finish immediately actionable work. Call update_goal with a completion audit only when verified complete. If the same genuine blocker persists, report the same concise reason once per turn. Use goal_wait for a justified later check. An observation timeout is not completion: inspect the existing live job, never duplicate it. Existing loop tools remain responsible for loop pacing.";

export class GoalRuntime {
  readonly #host: GoalHost;
  #goal: GoalState | null = null;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #removeTasks: (() => void) | undefined;
  #ticket: Ticket | null = null;
  #runId: string | null = null;
  #inRun: boolean = false;
  #startedAt: number = 0;
  #tokens: number = 0;
  #lastError: string | null = null;
  #observedWork: boolean = false;

  constructor(host: GoalHost) {
    this.#host = host;
  }

  get state(): GoalState | null {
    return this.#goal;
  }

  restore(entries: readonly unknown[]): void {
    this.shutdown();
    this.#goal = restoreGoal({
      entries,
      sessionId: this.#host.sessionId,
      now: Date.now(),
    });
    this.#removeTasks = subscribeTasks(this.#host.bus, (notice) =>
      this.#track(notice),
    );
    this.#host.display(this.#goal);
    this.#schedule();
  }

  start(input: StartInput): void {
    if (this.#goal !== null && this.#goal.status !== "complete")
      throw new Error("Clear or edit the existing unfinished goal first.");
    this.#save(
      createGoal({
        ...input,
        id: randomUUID(),
        sessionId: this.#host.sessionId,
        now: Date.now(),
      }),
    );
    this.#lastError = null;
    this.#schedule();
  }

  pause(): void {
    this.#save(pauseGoal(this.#require(), Date.now()));
  }

  resume(): void {
    this.#lastError = null;
    this.#save(resumeGoal(this.#require(), Date.now()));
    this.#schedule();
  }

  clear(): void {
    this.#save(null);
  }

  edit(objective: string): void {
    if (objective.trim().length === 0)
      throw new Error("Objective must not be empty.");
    const goal: GoalState = this.#require();
    this.#save({
      ...pauseGoal(goal, Date.now()),
      objective: objective.trim(),
      blocker: null,
      reason: "Objective edited; explicitly resume when ready.",
    });
  }

  budget(tokens: number | null): void {
    if (tokens !== null && (!Number.isSafeInteger(tokens) || tokens <= 0))
      throw new Error("Budget must be a positive safe integer or null.");
    const goal: GoalState = this.#require();
    this.#save({
      ...pauseGoal(goal, Date.now()),
      tokenBudget: tokens,
      reason: "Budget edited; explicitly resume when ready.",
    });
  }

  wait(input: WaitInput): void {
    if (
      !Number.isInteger(input.delaySeconds) ||
      input.delaySeconds < MIN_WAIT_SECONDS ||
      input.delaySeconds > MAX_WAIT_SECONDS ||
      input.reason.trim().length === 0
    )
      throw new Error("Wait requires a reason and 60–3,600 integer seconds.");
    const goal: GoalState = this.#active();
    this.#save({
      ...goal,
      revision: goal.revision + 1,
      wait: {
        until: Date.now() + input.delaySeconds * SECOND_MS,
        reason: input.reason.trim(),
      },
      updatedAt: Date.now(),
    });
    this.#schedule();
  }

  update(status: "complete" | "blocked", reason: string): void {
    const goal: GoalState = this.#active();
    if (status === "complete" && this.#hasLiveTasks(goal))
      throw new Error(
        "Inspect owned tmux tasks before claiming goal completion.",
      );
    this.#save(updateGoal({ goal, status, reason, now: Date.now() }));
  }

  accept(text: string): boolean {
    const valid: boolean =
      this.#goal?.status === "active" &&
      this.#ticket?.text === text &&
      this.#ticket.revision === this.#goal.revision;
    if (valid) this.#ticket = null;
    return valid;
  }

  invalidateTicket(): void {
    this.#ticket = null;
  }

  begin(): void {
    this.#inRun = true;
    this.#observedWork = false;
    this.#lastError = null;
    this.#tokens = 0;
    this.#runId = this.#goal?.status === "active" ? this.#goal.id : null;
    this.#startedAt = Date.now();
    if (this.#goal?.status === "active")
      this.#save({
        ...this.#goal,
        turn: this.#goal.turn + 1,
        updatedAt: Date.now(),
      });
  }

  recordTool(name: string): void {
    if (!CONTROL_TOOLS.has(name)) this.#observedWork = true;
  }

  recordTokens(tokens: number): void {
    if (!Number.isSafeInteger(tokens) || tokens <= 0) return;
    if (this.#inRun) {
      this.#tokens += tokens;
      return;
    }
    if (this.#goal?.status === "active") {
      this.#save(
        accountUsage({
          goal: this.#goal,
          tokens,
          elapsedMs: 0,
          now: Date.now(),
        }),
      );
    }
  }

  end(error: string | null): void {
    if (this.#goal === null || this.#runId !== this.#goal.id) {
      this.#resetRun();
      return;
    }
    this.#lastError = error;
    this.#save(
      accountUsage({
        goal: this.#goal,
        tokens: this.#tokens,
        elapsedMs: Math.max(0, Date.now() - this.#startedAt),
        now: Date.now(),
      }),
    );
    this.#resetRun();
    if (error === null) this.#auditProgress();
  }

  #resetRun(): void {
    this.#inRun = false;
    this.#tokens = 0;
    this.#runId = null;
  }

  #auditProgress(): void {
    const goal: GoalState | null = this.#goal;
    if (goal?.status !== "active") return;
    const activity = queryActivity(this.#host.bus, this.#host.sessionId);
    const waiting: boolean =
      (goal.wait !== null && goal.wait.until > Date.now()) ||
      activity.some(
        (snapshot) =>
          snapshot.tasks.some((name) => goal.tasks.includes(name)) ||
          snapshot.pendingDelivery,
      );
    const reportedBlocker: boolean = goal.blocker?.turn === goal.turn;
    const noProgressTurns: number =
      this.#observedWork || waiting || reportedBlocker
        ? 0
        : goal.noProgressTurns + 1;
    const blocked: boolean = noProgressTurns >= MAX_STALLED_TURNS;
    this.#save({
      ...goal,
      noProgressTurns,
      status: blocked ? "blocked" : goal.status,
      revision: blocked ? goal.revision + 1 : goal.revision,
      reason: blocked
        ? "Three consecutive goal turns ended without tool evidence or a verified wait."
        : goal.reason,
    });
  }

  settled(): void {
    if (this.#goal?.status === "active" && this.#lastError !== null)
      this.#stopWithReason(this.#lastError);
    this.#schedule();
  }

  shutdown(): void {
    this.#cancelTimer();
    this.#removeTasks?.();
    this.#removeTasks = undefined;
    this.#ticket = null;
    this.#resetRun();
    this.#lastError = null;
  }

  #require(): GoalState {
    if (this.#goal === null)
      throw new Error("No goal is configured. Use /goal <objective>.");
    return this.#goal;
  }

  #active(): GoalState {
    const goal: GoalState = this.#require();
    if (goal.status !== "active")
      throw new Error("Goal is not active; only the user can resume it.");
    return goal;
  }

  #save(goal: GoalState | null): void {
    this.#host.persist(GOAL_ENTRY, goal);
    this.#goal = goal;
    if (this.#ticket !== null && this.#ticket.revision !== goal?.revision)
      this.#ticket = null;
    if (goal?.status !== "active") {
      this.#ticket = null;
      this.#cancelTimer();
    }
    this.#host.display(goal);
  }

  #stopWithReason(reason: string): void {
    this.#save({ ...pauseGoal(this.#require(), Date.now()), reason });
  }

  #track(notice: TaskNotice): void {
    if (
      notice.sessionId !== this.#host.sessionId ||
      this.#goal?.status !== "active" ||
      this.#goal.tasks.includes(notice.name)
    )
      return;
    this.#save({
      ...this.#goal,
      tasks: [...this.#goal.tasks, notice.name],
      updatedAt: Date.now(),
    });
  }

  #hasLiveTasks(goal: GoalState): boolean {
    const tmux = queryActivity(this.#host.bus, this.#host.sessionId).find(
      (snapshot) => snapshot.source === "tmux",
    );
    if (goal.tasks.length > 0 && tmux === undefined)
      throw new Error(
        "Owned task monitoring is unavailable; enable the tmux extension before continuing.",
      );
    return (
      tmux !== undefined &&
      (goal.tasks.some((name) => tmux.tasks.includes(name)) ||
        (goal.tasks.length > 0 && tmux.pendingDelivery))
    );
  }

  #cancelTimer(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
  }

  #schedule(): void {
    this.#cancelTimer();
    if (this.#goal?.status !== "active") return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      this.#poll();
    }, POLL_MS);
    this.#timer.unref();
  }

  #poll(): void {
    try {
      this.#deliver();
    } catch (error: unknown) {
      this.#stopWithReason(
        error instanceof Error ? error.message : "Goal continuation failed.",
      );
    }
    this.#schedule();
  }

  #deliver(): void {
    const goal: GoalState | null = this.#goal;
    if (goal?.status !== "active" || !this.#host.isReady()) return;
    if (this.#ticket !== null) {
      if (Date.now() - this.#ticket.createdAt >= ACCEPT_TIMEOUT_MS)
        this.#stopWithReason(
          "Goal continuation was not accepted; inspect the error and explicitly resume.",
        );
      return;
    }
    const activity = queryActivity(this.#host.bus, this.#host.sessionId);
    if (
      activity.some(
        (snapshot) => snapshot.ownsContinuation || snapshot.pendingDelivery,
      )
    )
      return;
    if (goal.wait !== null && goal.wait.until > Date.now()) return;
    if (this.#hasLiveTasks(goal)) return;
    this.#ticket = {
      text: `${GOAL_MESSAGE_PREFIX}${randomUUID()}]\n${CONTINUATION}`,
      revision: goal.revision,
      createdAt: Date.now(),
    };
    try {
      this.#host.send(this.#ticket.text);
    } catch (error: unknown) {
      this.#ticket = null;
      if (!(error instanceof Error && error.message.includes(BUSY_ERROR)))
        throw error;
    }
  }
}
