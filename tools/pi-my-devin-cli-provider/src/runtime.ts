// This file runs with Bun.
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import process from "node:process";
import { Readable, Writable } from "node:stream";
import {
  client,
  methods,
  ndJsonStream,
  PROTOCOL_VERSION,
  type ActiveSession,
  type ActiveSessionMessage,
  type ClientContext,
  type RequestPermissionRequest,
  type RequestPermissionResponse,
  type SessionUpdate,
} from "@agentclientprotocol/sdk";
import { transcriptIncludesCompaction } from "./context.ts";

interface ErrorState {
  stderr: string;
}

interface RuntimeJob {
  continuationPrompt: string;
  cwd: string;
  initialPrompt: string;
  modelId: string;
  onUpdate: (update: SessionUpdate) => void;
  sessionId: string;
  signal: AbortSignal | undefined;
}

interface JobResult {
  error: Error | undefined;
}

interface PendingJob {
  job: RuntimeJob;
  resolve: (result: JobResult) => void;
}

interface RuntimeHandles {
  context: ClientContext;
  take: () => Promise<PendingJob | undefined>;
}

interface SharedRuntime {
  activeJobs: number;
  acpSession: ActiveSession | undefined;
  child: ChildProcessWithoutNullStreams;
  context: ClientContext | undefined;
  errorState: ErrorState;
  idleTimer: ReturnType<typeof setTimeout> | undefined;
  key: string;
  push: (pending: PendingJob) => void;
  ready: Promise<RuntimeHandles>;
  stop: () => void;
  turnCount: number;
}

interface RuntimePoolState {
  idleTtlMs: number;
  runtimes: Map<string, SharedRuntime>;
}

interface DeferredHandles {
  promise: Promise<RuntimeHandles>;
  reject: (error: unknown) => void;
  resolve: (value: RuntimeHandles) => void;
}

const DEVIN_COMMAND: string = "devin";
const DEVIN_PERMISSION_MODE: string = "dangerous";
const CLIENT_NAME: string = "pi-my-devin-cli-provider";
const CLIENT_VERSION: string = "0.1.0";
const STDERR_LIMIT: number = 8_192;
const DEFAULT_IDLE_TTL_MS: number = 120_000;
const MODEL_CONFIG_ID: string = "model";
const BYPASS_MODE_ID: string = "bypass";
const KEY_SEPARATOR: string = "\0";
const SESSION_ID_PREFIX: string = "devin-pi:";
const poolState: RuntimePoolState = {
  idleTtlMs: DEFAULT_IDLE_TTL_MS,
  runtimes: new Map(),
};

export function selectPermission(request: RequestPermissionRequest): RequestPermissionResponse {
  const preferred =
    request.options.find((option) => option.kind === "allow_always") ??
    request.options.find((option) => option.kind === "allow_once") ??
    request.options[0];
  return preferred
    ? { outcome: { outcome: "selected", optionId: preferred.optionId } }
    : { outcome: { outcome: "cancelled" } };
}

export function createDevinSessionId(): string {
  return `${SESSION_ID_PREFIX}${randomUUID()}`;
}

export function resolveDevinSessionId(sessionId: string | undefined): string {
  return sessionId && sessionId.length > 0 ? sessionId : createDevinSessionId();
}

export function runtimeKey(cwd: string, modelId: string, sessionId: string): string {
  return `${cwd}${KEY_SEPARATOR}${modelId}${KEY_SEPARATOR}${sessionId}`;
}

export function invalidateDevinSessionsForPiSession(sessionId: string): void {
  if (sessionId.length === 0) return;
  [...poolState.runtimes.values()]
    .filter((runtime) => runtime.key.endsWith(`${KEY_SEPARATOR}${sessionId}`))
    .forEach((runtime) => runtime.stop());
}

function clearIdleTimer(runtime: SharedRuntime): void {
  if (!runtime.idleTimer) return;
  clearTimeout(runtime.idleTimer);
  runtime.idleTimer = undefined;
}

function scheduleIdleStop(runtime: SharedRuntime): void {
  clearIdleTimer(runtime);
  runtime.idleTimer = setTimeout(() => {
    if (runtime.activeJobs === 0) runtime.stop();
  }, poolState.idleTtlMs);
  runtime.idleTimer.unref();
}

function attachStderr(child: ChildProcessWithoutNullStreams): ErrorState {
  const state: ErrorState = { stderr: "" };
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk: string) => {
    state.stderr = `${state.stderr}${chunk}`.slice(-STDERR_LIMIT);
  });
  return state;
}

function stdoutByteStream(stdout: NodeJS.ReadableStream): ReadableStream<Uint8Array> {
  const reader = Readable.toWeb(stdout).getReader();
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      const result = await reader.read();
      if (result.done) {
        controller.close();
        return;
      }
      const value: unknown = result.value;
      controller.enqueue(value instanceof Uint8Array ? value : new Uint8Array());
    },
    cancel: () => reader.cancel(),
  });
}

function formatError(error: unknown, stderr: string): Error {
  const detail: string = error instanceof Error ? error.message : String(error);
  const suffix: string = stderr.trim();
  return new Error(suffix.length > 0 ? `${detail}: ${suffix}` : detail);
}

async function configureSession(
  context: ClientContext,
  session: ActiveSession,
  modelId: string,
): Promise<void> {
  await context
    .request(methods.agent.session.setConfigOption, {
      sessionId: session.sessionId,
      configId: MODEL_CONFIG_ID,
      value: modelId,
    })
    .catch(() => undefined);
  const bypass = session.modes?.availableModes.find((mode) => mode.id === BYPASS_MODE_ID);
  if (!bypass) return;
  await context
    .request(methods.agent.session.setMode, {
      sessionId: session.sessionId,
      modeId: bypass.id,
    })
    .catch(() => undefined);
}

async function consumeUpdates(
  session: ActiveSession,
  onUpdate: (update: SessionUpdate) => void,
): Promise<void> {
  const message: ActiveSessionMessage = await session.nextUpdate();
  if (message.kind === "stop") return;
  onUpdate(message.update);
  return consumeUpdates(session, onUpdate);
}

function closeAcpSession(runtime: SharedRuntime): void {
  const session = runtime.acpSession;
  const context = runtime.context;
  runtime.acpSession = undefined;
  runtime.turnCount = 0;
  if (!session) return;
  session.dispose();
  if (!context) return;
  void context
    .request(methods.agent.session.delete, { sessionId: session.sessionId })
    .catch(() => undefined);
}

async function runJob(
  runtime: SharedRuntime,
  context: ClientContext,
  job: RuntimeJob,
): Promise<void> {
  if (job.signal?.aborted) {
    throw job.signal.reason instanceof Error
      ? job.signal.reason
      : new Error("Devin ACP request aborted");
  }
  if (transcriptIncludesCompaction(job.initialPrompt)) {
    closeAcpSession(runtime);
  }
  if (!runtime.acpSession) {
    runtime.acpSession = await context.buildSession({ cwd: job.cwd, mcpServers: [] }).start();
    await configureSession(context, runtime.acpSession, job.modelId);
  }
  const session = runtime.acpSession;
  const promptText: string = runtime.turnCount === 0 ? job.initialPrompt : job.continuationPrompt;
  runtime.turnCount += 1;
  const cancel = (): void => {
    void context.notify(methods.agent.session.cancel, { sessionId: session.sessionId });
  };
  job.signal?.addEventListener("abort", cancel, { once: true });
  try {
    void session.prompt(promptText);
    await consumeUpdates(session, job.onUpdate);
  } finally {
    job.signal?.removeEventListener("abort", cancel);
  }
}

function createQueue(): {
  push: (pending: PendingJob) => void;
  take: () => Promise<PendingJob | undefined>;
  close: () => void;
} {
  const waiting: Array<(value: PendingJob | undefined) => void> = [];
  const queued: PendingJob[] = [];
  const closed: { value: boolean } = { value: false };
  return {
    push: (pending) => {
      if (closed.value) {
        pending.resolve({ error: new Error("Devin ACP runtime is stopped") });
        return;
      }
      const waiter = waiting.shift();
      if (waiter) {
        waiter(pending);
        return;
      }
      queued.push(pending);
    },
    take: () => {
      if (queued.length > 0) return Promise.resolve(queued.shift());
      if (closed.value) return Promise.resolve(undefined);
      return new Promise((resolve) => {
        waiting.push(resolve);
      });
    },
    close: () => {
      closed.value = true;
      waiting.splice(0).forEach((resolve) => resolve(undefined));
      queued.splice(0).forEach((pending) => {
        pending.resolve({ error: new Error("Devin ACP runtime is stopped") });
      });
    },
  };
}

function forgetRuntime(runtime: SharedRuntime): void {
  if (poolState.runtimes.get(runtime.key) === runtime) {
    poolState.runtimes.delete(runtime.key);
  }
}

function createDeferredHandles(): DeferredHandles {
  const holders: {
    reject: ((error: unknown) => void) | undefined;
    resolve: ((value: RuntimeHandles) => void) | undefined;
  } = { reject: undefined, resolve: undefined };
  return {
    promise: new Promise<RuntimeHandles>((resolve, reject) => {
      holders.resolve = resolve;
      holders.reject = reject;
    }),
    resolve: (value) => {
      holders.resolve?.(value);
    },
    reject: (error) => {
      holders.reject?.(error);
    },
  };
}

function startSharedRuntime(key: string): SharedRuntime {
  const child: ChildProcessWithoutNullStreams = spawn(DEVIN_COMMAND, ["acp"], {
    cwd: process.cwd(),
    env: { ...process.env, DEVIN_PERMISSION_MODE },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.unref?.();
  const errorState: ErrorState = attachStderr(child);
  const queue = createQueue();
  const deferred = createDeferredHandles();
  const stopped: { value: boolean } = { value: false };
  const stop = (): void => {
    if (stopped.value) return;
    stopped.value = true;
    clearIdleTimer(runtime);
    forgetRuntime(runtime);
    closeAcpSession(runtime);
    queue.close();
    child.stdin.end();
    if (child.exitCode === null) child.kill();
  };
  const runtime: SharedRuntime = {
    activeJobs: 0,
    acpSession: undefined,
    child,
    context: undefined,
    errorState,
    idleTimer: undefined,
    key,
    push: queue.push,
    ready: deferred.promise,
    stop,
    turnCount: 0,
  };
  const stream = ndJsonStream(Writable.toWeb(child.stdin), stdoutByteStream(child.stdout));
  void client({ name: CLIENT_NAME })
    .onRequest(methods.client.session.requestPermission, ({ params }) => selectPermission(params))
    .connectWith(stream, async (context) => {
      await context.request(methods.agent.initialize, {
        protocolVersion: PROTOCOL_VERSION,
        clientCapabilities: {},
        clientInfo: { name: CLIENT_NAME, version: CLIENT_VERSION },
      });
      runtime.context = context;
      deferred.resolve({ context, take: queue.take });
      const processJobs = async (): Promise<void> => {
        const pending = await queue.take();
        if (!pending) return;
        try {
          await runJob(runtime, context, pending.job);
          pending.resolve({ error: undefined });
        } catch (error) {
          pending.resolve({ error: formatError(error, errorState.stderr) });
        }
        return processJobs();
      };
      await processJobs();
    })
    .catch((error: unknown) => {
      deferred.reject(formatError(error, errorState.stderr));
      stop();
    });
  child.once("exit", () => {
    stop();
  });
  child.once("error", () => {
    stop();
  });
  return runtime;
}

function acquireRuntime(key: string): SharedRuntime {
  const existing = poolState.runtimes.get(key);
  if (existing && existing.child.exitCode === null) {
    clearIdleTimer(existing);
    existing.activeJobs += 1;
    return existing;
  }
  const runtime = startSharedRuntime(key);
  runtime.activeJobs = 1;
  poolState.runtimes.set(key, runtime);
  return runtime;
}

function releaseRuntime(runtime: SharedRuntime): void {
  runtime.activeJobs = Math.max(0, runtime.activeJobs - 1);
  if (runtime.activeJobs > 0) return;
  if (isPrintMode()) {
    runtime.stop();
    return;
  }
  scheduleIdleStop(runtime);
}

function isPrintMode(): boolean {
  return process.argv.includes("-p") || process.argv.includes("--print");
}

export async function runDevinJob(job: RuntimeJob): Promise<void> {
  const key = runtimeKey(job.cwd, job.modelId, job.sessionId);
  const runtime = acquireRuntime(key);
  try {
    await runtime.ready;
    const result: JobResult = await new Promise((resolve) => {
      runtime.push({ job, resolve });
    });
    if (result.error) throw result.error;
  } catch (error) {
    runtime.stop();
    throw error instanceof Error ? error : formatError(error, runtime.errorState.stderr);
  } finally {
    if (runtime.child.exitCode === null) releaseRuntime(runtime);
  }
}

process.on("beforeExit", () => {
  [...poolState.runtimes.values()].forEach((runtime) => runtime.stop());
});

export const devinRuntimeTestApi = {
  idleTtlMs: (): number => poolState.idleTtlMs,
  isRunning: (): boolean =>
    [...poolState.runtimes.values()].some((runtime) => runtime.child.exitCode === null),
  pooledCount: (): number => poolState.runtimes.size,
  reset(): void {
    [...poolState.runtimes.values()].forEach((runtime) => {
      clearIdleTimer(runtime);
      runtime.stop();
    });
    poolState.runtimes.clear();
    poolState.idleTtlMs = DEFAULT_IDLE_TTL_MS;
  },
  setIdleTtlMs(ms: number): void {
    poolState.idleTtlMs = ms;
  },
  stop(): void {
    [...poolState.runtimes.values()].forEach((runtime) => runtime.stop());
  },
};
