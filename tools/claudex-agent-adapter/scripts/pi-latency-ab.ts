#!/usr/bin/env bun

// Measure direct versus Pi SSE latency from the repository root; use --mode=text or --mode=tool.
// Example: bun tools/claudex-agent-adapter/scripts/pi-latency-ab.ts --mode=text

export {};

const EXPECTED_BUILD_ID = requiredArgument("--expected-build-id", "df0eb750ef71c1f4");
const MODEL = requiredArgument("--model", "gpt-5.6-luna");
const DIRECT_BACKEND = requiredArgument("--direct-backend", "codex-app-server");
const PI_PROVIDER = requiredArgument("--pi-provider", "openai-codex");
const PI_MODEL = requiredArgument("--pi-model", MODEL);
const ROUTE_FILTER = requiredArgument("--route", "both");
const MODE = requiredArgument("--mode", "tool");
const DEBUG_PI_EVENTS = process.argv.includes("--debug-pi-events");
const SAMPLE_COUNT = Number(requiredArgument("--sample-count", "5"));
const FOLLOW_UP_DELAY_MS = Number(requiredArgument("--follow-up-delay-ms", "200"));
const DIRECT_PORT = 18431;
const PI_PORT = 18432;
const SESSION_HEADER = "x-claude-code-session-id";
const STARTUP_TIMEOUT_MS = 10_000;
const REQUEST_TIMEOUT_MS = 180_000;

interface RouteConfig {
  label: "direct" | "pi";
  port: number;
  providerInterface?: "pi";
}

interface StreamTiming {
  sentAtMs: number;
  firstFrameAtMs: number;
  firstDeltaAtMs: number | null;
  firstTextDeltaAtMs: number | null;
  messageStopAtMs: number;
  sseChunkCount: number;
  sseFrameCount: number;
  contentDeltaAtMs: number[];
  contentDeltaChunkIndexes: number[];
  eventTypeCounts: Record<string, number>;
  deltaTypeCounts: Record<string, number>;
  firstDeltaByTypeAtMs: Record<string, number>;
  toolUseStartAtMs: number | null;
  responseText: string;
  toolUseCount: number;
}

interface StreamResult {
  timing: StreamTiming;
  toolUseId?: string;
  toolInput?: Record<string, unknown>;
}

interface MetricRow extends StreamTiming {
  route: RouteConfig["label"];
  sample: number;
  coldSpawn: boolean;
  firstPair: boolean;
  phase: "text" | "tool_request" | "tool_result";
  valid: boolean;
  validationError?: string;
  firstFrameMs: number;
  firstDeltaMs: number | null;
  firstTextDeltaMs: number | null;
  messageStopMs: number;
  contentDeltaRelativeMs: number[];
  contentDeltaIntervalsMs: number[];
}

interface SummaryRow {
  route: RouteConfig["label"];
  phase: MetricRow["phase"];
  temperature: "cold_spawn" | "first_pair_follow_up" | "warm";
  count: number;
  firstFrameMedianMs: number;
  firstDeltaMedianMs: number | null;
  firstTextDeltaMedianMs: number | null;
  messageStopMedianMs: number;
}

interface RunningAdapter {
  config: RouteConfig;
  process: ReturnType<typeof Bun.spawn>;
  stderr: Promise<string>;
}

const ALL_ROUTES: RouteConfig[] = [
  { label: "direct", port: DIRECT_PORT },
  { label: "pi", port: PI_PORT, providerInterface: "pi" },
];
const ROUTES = ALL_ROUTES.filter(
  (route) => ROUTE_FILTER === "both" || route.label === ROUTE_FILTER,
);
if (ROUTES.length === 0) {
  throw new Error(`--route must be direct, pi, or both; received ${ROUTE_FILTER}`);
}
if (MODE !== "text" && MODE !== "tool") {
  throw new Error(`--mode must be text or tool; received ${MODE}`);
}
if (!Number.isInteger(SAMPLE_COUNT) || SAMPLE_COUNT < 1) {
  throw new Error(`--sample-count must be a positive integer; received ${SAMPLE_COUNT}`);
}
if (!Number.isFinite(FOLLOW_UP_DELAY_MS) || FOLLOW_UP_DELAY_MS < 0) {
  throw new Error(`--follow-up-delay-ms must be non-negative; received ${FOLLOW_UP_DELAY_MS}`);
}

const INITIAL_BODY: Record<string, unknown> =
  MODE === "text"
    ? {
        model: MODEL,
        max_tokens: 64,
        stream: true,
        messages: [{ role: "user", content: "Reply with exactly LATENCY_TEXT_OK." }],
      }
    : {
        model: MODEL,
        max_tokens: 256,
        stream: true,
        messages: [
          {
            role: "user",
            content:
              'Call latency_probe exactly once with {"nonce":"LATENCY_NONCE"}. Do not answer in text before the tool call. After its result, answer only LATENCY_TOOL_LOOP_OK.',
          },
        ],
        tools: [
          {
            name: "latency_probe",
            description: "A deterministic no-op latency measurement tool.",
            input_schema: {
              type: "object",
              properties: { nonce: { type: "string" } },
              required: ["nonce"],
              additionalProperties: false,
            },
          },
        ],
      };

function requiredArgument(name: string, fallback?: string): string {
  const prefix = `${name}=`;
  const value = process.argv
    .slice(2)
    .find((argument) => argument.startsWith(prefix))
    ?.slice(prefix.length);
  if (value !== undefined && value.length > 0) {
    return value;
  }
  if (fallback !== undefined) {
    return fallback;
  }
  throw new Error(`Missing required argument ${name}=VALUE`);
}

function epochMilliseconds(): number {
  return performance.timeOrigin + performance.now();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function collectStream(stream: ReadableStream<Uint8Array>): Promise<string> {
  return new Response(stream).text();
}

async function waitForHealth(config: RouteConfig): Promise<Record<string, unknown>> {
  const deadline = performance.now() + STARTUP_TIMEOUT_MS;
  while (performance.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${config.port}/health`);
      if (response.ok) {
        const value: unknown = await response.json();
        if (isRecord(value)) {
          return value;
        }
      }
    } catch {
      // The listener is expected to reject connections during startup.
    }
    await Bun.sleep(25);
  }
  throw new Error(`${config.label} adapter did not become healthy`);
}

function modelRoute(health: Record<string, unknown>): Record<string, unknown> {
  const routes = health.backend_routes;
  if (!Array.isArray(routes)) {
    throw new Error("Health response omitted backend_routes");
  }
  for (const route of routes) {
    if (typeof route !== "string" || !route.startsWith("{")) {
      continue;
    }
    const value: unknown = JSON.parse(route);
    if (isRecord(value) && value.model === MODEL) {
      return value;
    }
  }
  throw new Error(`Health response omitted the ${MODEL} route`);
}

function assertHealth(config: RouteConfig, health: Record<string, unknown>): void {
  if (health.build_id !== EXPECTED_BUILD_ID) {
    throw new Error(
      `${config.label} build ID was ${String(health.build_id)}, expected ${EXPECTED_BUILD_ID}`,
    );
  }
  const route = modelRoute(health);
  const expectedBackend = config.label === "pi" ? "pi-gateway" : DIRECT_BACKEND;
  if (route.backend !== expectedBackend) {
    throw new Error(
      `${config.label} backend was ${String(route.backend)}, expected ${expectedBackend}`,
    );
  }
  if (config.label === "pi") {
    if (route.piProvider !== PI_PROVIDER || route.piModel !== PI_MODEL) {
      throw new Error(`Pi route mapping mismatch: ${JSON.stringify(route)}`);
    }
  } else if (route.piProvider !== undefined || route.piModel !== undefined) {
    throw new Error(`Direct route unexpectedly retained Pi mapping: ${JSON.stringify(route)}`);
  }
}

function startupRoutes(log: string): string {
  for (const line of log.split("\n")) {
    try {
      const value: unknown = JSON.parse(line);
      if (
        isRecord(value) &&
        value.message === "claudex agent adapter is ready" &&
        typeof value.routes === "string"
      ) {
        return value.routes;
      }
    } catch {
      // Ignore non-JSON provider diagnostics after the structured startup line.
    }
  }
  throw new Error(`Adapter startup log omitted its ready line: ${log}`);
}

function directBackendLogName(): string {
  if (DIRECT_BACKEND === "codex-app-server") {
    return "CodexAppServer";
  }
  if (DIRECT_BACKEND === "configured-acp") {
    return "ConfiguredAcp";
  }
  throw new Error(`Unsupported direct backend assertion: ${DIRECT_BACKEND}`);
}

function assertStartupLog(config: RouteConfig, log: string): void {
  const routes = startupRoutes(log);
  const backend =
    config.label === "pi" ? "backend: PiGateway" : `backend: ${directBackendLogName()}`;
  const provider =
    config.label === "pi" ? `pi_provider: Some("${PI_PROVIDER}")` : "pi_provider: None";
  if (!routes.includes(backend) || !routes.includes(provider)) {
    throw new Error(`${config.label} startup log did not prove its route: ${routes}`);
  }
}

function adapterEnvironment(): Record<string, string | undefined> {
  const environment = { ...process.env };
  delete environment.ANTHROPIC_AUTH_TOKEN;
  if (DEBUG_PI_EVENTS) {
    environment.RUST_LOG = "claudex_agent_adapter::pi_gateway=debug";
  }
  return environment;
}

function startAdapter(binary: string, providerConfig: string, config: RouteConfig): RunningAdapter {
  const argumentsList = [
    binary,
    "serve",
    "--provider-config",
    providerConfig,
    "--model",
    MODEL,
    "--listen",
    `127.0.0.1:${config.port}`,
  ];
  if (config.providerInterface !== undefined) {
    argumentsList.push("--provider-interface", config.providerInterface);
  }
  const child = Bun.spawn(argumentsList, {
    env: adapterEnvironment(),
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  void collectStream(child.stdout);
  return { config, process: child, stderr: collectStream(child.stderr) };
}

function parseEvent(frame: string): { event?: string; data?: Record<string, unknown> } {
  const lines = frame.split("\n");
  const event = lines
    .find((line) => line.startsWith("event:"))
    ?.slice("event:".length)
    .trim();
  const dataText = lines
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice("data:".length).trimStart())
    .join("\n");
  if (dataText.length === 0) {
    return { event };
  }
  const value: unknown = JSON.parse(dataText);
  return isRecord(value) ? { event, data: value } : { event };
}

function toolIdFromEvent(event: {
  event?: string;
  data?: Record<string, unknown>;
}): string | undefined {
  if (event.event !== "content_block_start" || event.data === undefined) {
    return undefined;
  }
  const block = event.data.content_block;
  if (!isRecord(block) || block.type !== "tool_use" || block.name !== "latency_probe") {
    return undefined;
  }
  return typeof block.id === "string" ? block.id : undefined;
}

async function streamRequest(
  url: string,
  sessionId: string,
  body: Record<string, unknown>,
): Promise<StreamResult> {
  const sentAtMs = epochMilliseconds();
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", [SESSION_HEADER]: sessionId },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  if (!response.ok || response.body === null) {
    throw new Error(`POST ${url} failed with ${response.status}: ${await response.text()}`);
  }
  if (!response.headers.get("content-type")?.startsWith("text/event-stream")) {
    throw new Error(`POST ${url} did not return text/event-stream`);
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let firstFrameAtMs: number | undefined;
  let firstDeltaAtMs: number | undefined;
  let messageStopAtMs: number | undefined;
  let toolUseId: string | undefined;
  let toolUseCount = 0;
  let toolUseStartAtMs: number | null = null;
  let sseChunkCount = 0;
  let sseFrameCount = 0;
  let responseText = "";
  let toolInputJson = "";
  let toolStartInput: Record<string, unknown> | undefined;
  const contentDeltaAtMs: number[] = [];
  const contentDeltaChunkIndexes: number[] = [];
  const eventTypeCounts: Record<string, number> = {};
  const deltaTypeCounts: Record<string, number> = {};
  const firstDeltaByTypeAtMs: Record<string, number> = {};
  while (messageStopAtMs === undefined) {
    const chunk = await reader.read();
    if (chunk.done) {
      buffer += decoder.decode();
      break;
    }
    sseChunkCount += 1;
    const observedAtMs = epochMilliseconds();
    buffer = `${buffer}${decoder.decode(chunk.value, { stream: true })}`.replaceAll("\r\n", "\n");
    let boundary = buffer.indexOf("\n\n");
    while (boundary >= 0) {
      const frame = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      sseFrameCount += 1;
      firstFrameAtMs ??= observedAtMs;
      const event = parseEvent(frame);
      if (event.event !== undefined) {
        eventTypeCounts[event.event] = (eventTypeCounts[event.event] ?? 0) + 1;
      }
      if (event.event === "content_block_delta") {
        firstDeltaAtMs ??= observedAtMs;
        contentDeltaAtMs.push(observedAtMs);
        contentDeltaChunkIndexes.push(sseChunkCount);
        const delta = event.data?.delta;
        if (isRecord(delta) && typeof delta.type === "string") {
          deltaTypeCounts[delta.type] = (deltaTypeCounts[delta.type] ?? 0) + 1;
          firstDeltaByTypeAtMs[delta.type] ??= observedAtMs;
        }
        if (isRecord(delta) && delta.type === "text_delta" && typeof delta.text === "string") {
          responseText += delta.text;
        }
        if (
          isRecord(delta) &&
          delta.type === "input_json_delta" &&
          typeof delta.partial_json === "string"
        ) {
          toolInputJson += delta.partial_json;
        }
      }
      const observedToolId = toolIdFromEvent(event);
      if (observedToolId !== undefined) {
        toolUseCount += 1;
        toolUseStartAtMs ??= observedAtMs;
        toolUseId ??= observedToolId;
        const input = event.data?.content_block;
        if (isRecord(input) && isRecord(input.input)) {
          toolStartInput = input.input;
        }
      }
      if (event.event === "message_stop") {
        messageStopAtMs = observedAtMs;
        break;
      }
      boundary = buffer.indexOf("\n\n");
    }
  }
  await reader.cancel();
  if (firstFrameAtMs === undefined || messageStopAtMs === undefined) {
    throw new Error(
      `Incomplete SSE stream from ${url}: ${JSON.stringify({ sseChunkCount, sseFrameCount, eventTypeCounts })}`,
    );
  }
  const parsedToolInput: unknown =
    toolInputJson.length > 0 ? JSON.parse(toolInputJson) : toolStartInput;
  return {
    timing: {
      sentAtMs,
      firstFrameAtMs,
      firstDeltaAtMs: firstDeltaAtMs ?? null,
      firstTextDeltaAtMs: firstDeltaByTypeAtMs.text_delta ?? null,
      messageStopAtMs,
      sseChunkCount,
      sseFrameCount,
      contentDeltaAtMs,
      contentDeltaChunkIndexes,
      eventTypeCounts,
      deltaTypeCounts,
      firstDeltaByTypeAtMs,
      toolUseStartAtMs,
      responseText,
      toolUseCount,
    },
    toolUseId,
    ...(isRecord(parsedToolInput) ? { toolInput: parsedToolInput } : {}),
  };
}

function metric(
  config: RouteConfig,
  sample: number,
  phase: MetricRow["phase"],
  timing: StreamTiming,
): MetricRow {
  const contentDeltaRelativeMs = timing.contentDeltaAtMs.map(
    (timestamp) => timestamp - timing.sentAtMs,
  );
  return {
    route: config.label,
    sample,
    coldSpawn: sample === 1 && phase === "tool_request",
    firstPair: sample === 1,
    phase,
    valid: true,
    ...timing,
    firstFrameMs: timing.firstFrameAtMs - timing.sentAtMs,
    firstDeltaMs: timing.firstDeltaAtMs === null ? null : timing.firstDeltaAtMs - timing.sentAtMs,
    firstTextDeltaMs:
      timing.firstTextDeltaAtMs === null ? null : timing.firstTextDeltaAtMs - timing.sentAtMs,
    messageStopMs: timing.messageStopAtMs - timing.sentAtMs,
    contentDeltaRelativeMs,
    contentDeltaIntervalsMs: contentDeltaRelativeMs
      .slice(1)
      .map((timestamp, index) => timestamp - (contentDeltaRelativeMs[index] ?? timestamp)),
  };
}

function followUpBody(
  toolUseId: string,
  toolInput: Record<string, unknown>,
): Record<string, unknown> {
  return {
    ...INITIAL_BODY,
    messages: [
      INITIAL_BODY.messages,
      {
        role: "assistant",
        content: [{ type: "tool_use", id: toolUseId, name: "latency_probe", input: toolInput }],
      },
      {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: toolUseId, content: "LATENCY_PROBE_RESULT_OK" },
        ],
      },
    ].flat(),
  };
}

async function measureSample(config: RouteConfig, sample: number): Promise<MetricRow[]> {
  const url = `http://127.0.0.1:${config.port}/v1/messages`;
  const sessionId = `pi-latency-ab-${config.label}`;
  const initial = await streamRequest(url, sessionId, INITIAL_BODY);
  if (MODE === "text") {
    if (initial.timing.responseText.trim() !== "LATENCY_TEXT_OK") {
      throw new Error(
        `${config.label} sample ${sample} invalid text response: ${JSON.stringify(initial)}`,
      );
    }
    return [metric(config, sample, "text", initial.timing)];
  }
  if (
    initial.toolUseId === undefined ||
    initial.toolInput?.nonce !== "LATENCY_NONCE" ||
    initial.timing.toolUseCount !== 1 ||
    initial.timing.responseText.length !== 0
  ) {
    throw new Error(
      `${config.label} sample ${sample} invalid tool call: ${JSON.stringify(initial)}`,
    );
  }
  if (FOLLOW_UP_DELAY_MS > 0) {
    await Bun.sleep(FOLLOW_UP_DELAY_MS);
  }
  const followUp = await streamRequest(
    url,
    sessionId,
    followUpBody(initial.toolUseId, initial.toolInput),
  );
  if (followUp.timing.responseText.trim() !== "LATENCY_TOOL_LOOP_OK") {
    throw new Error(
      `${config.label} sample ${sample} invalid final text: ${JSON.stringify(followUp)}`,
    );
  }
  return [
    metric(config, sample, "tool_request", initial.timing),
    metric(config, sample, "tool_result", followUp.timing),
  ];
}

function median(values: number[]): number {
  const sorted = values.toSorted((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  const value = sorted[middle];
  if (value === undefined) {
    throw new Error("Cannot calculate the median of an empty sample");
  }
  if (sorted.length % 2 === 1) {
    return value;
  }
  const lower = sorted[middle - 1];
  if (lower === undefined) {
    throw new Error("Median lower bound is missing");
  }
  return (lower + value) / 2;
}

function optionalMedian(values: Array<number | null>): number | null {
  const observed = values.filter((value): value is number => value !== null);
  return observed.length === 0 ? null : median(observed);
}

function summarize(rows: MetricRow[]): SummaryRow[] {
  const summaries: SummaryRow[] = [];
  for (const route of ROUTES) {
    const phases: MetricRow["phase"][] =
      MODE === "text" ? ["text"] : ["tool_request", "tool_result"];
    for (const phase of phases) {
      const temperatures: SummaryRow["temperature"][] =
        phase === "tool_result" ? ["first_pair_follow_up", "warm"] : ["cold_spawn", "warm"];
      for (const temperature of temperatures) {
        const selected = rows.filter((row) => {
          if (row.route !== route.label || row.phase !== phase) {
            return false;
          }
          if (temperature === "cold_spawn") {
            return row.coldSpawn;
          }
          if (temperature === "first_pair_follow_up") {
            return row.firstPair;
          }
          return !row.firstPair;
        });
        if (selected.length === 0) {
          continue;
        }
        summaries.push({
          route: route.label,
          phase,
          temperature,
          count: selected.length,
          firstFrameMedianMs: median(selected.map((row) => row.firstFrameMs)),
          firstDeltaMedianMs: optionalMedian(selected.map((row) => row.firstDeltaMs)),
          firstTextDeltaMedianMs: optionalMedian(selected.map((row) => row.firstTextDeltaMs)),
          messageStopMedianMs: median(selected.map((row) => row.messageStopMs)),
        });
      }
    }
  }
  return summaries;
}

async function stopAdapter(adapter: RunningAdapter): Promise<string> {
  adapter.process.kill("SIGTERM");
  await adapter.process.exited;
  return adapter.stderr;
}

async function stopAndValidateAdapters(adapters: RunningAdapter[]): Promise<void> {
  for (const adapter of adapters) {
    const log = await stopAdapter(adapter);
    const artifact = `/tmp/claudex-pi-latency-${MODEL}-${adapter.config.label}.stderr.log`;
    await Bun.write(artifact, log);
    assertStartupLog(adapter.config, log);
  }
}

async function main(): Promise<void> {
  const binary = requiredArgument(
    "--binary",
    `${process.env.HOME}/.cargo/bin/claudex-agent-adapter`,
  );
  const providerConfig = requiredArgument(
    "--provider-config",
    `${process.cwd()}/.config/claudex/providers.json`,
  );
  const startupOnly = process.argv.includes("--startup-only");
  const buildId = (await Bun.$`${binary} build-id`.text()).trim();
  if (buildId !== EXPECTED_BUILD_ID) {
    throw new Error(`Harness requires installed build ${EXPECTED_BUILD_ID}, received ${buildId}`);
  }

  const adapters = ROUTES.map((config) => startAdapter(binary, providerConfig, config));
  const rows: MetricRow[] = [];
  try {
    for (const config of ROUTES) {
      assertHealth(config, await waitForHealth(config));
    }
    if (!startupOnly) {
      for (let sample = 1; sample <= SAMPLE_COUNT; sample += 1) {
        for (const config of ROUTES) {
          const sampleRows = await measureSample(config, sample);
          rows.push(...sampleRows);
          for (const row of sampleRows) {
            console.log(JSON.stringify({ type: "sample", ...row }));
          }
        }
      }
    }
  } catch (error) {
    console.error(
      JSON.stringify({
        type: "invalid_sample",
        model: MODEL,
        message: error instanceof Error ? error.message : String(error),
      }),
    );
    throw error;
  } finally {
    await stopAndValidateAdapters(adapters);
  }
  if (startupOnly) {
    console.log(JSON.stringify({ type: "startup_validation", status: "ok", buildId }));
    return;
  }
  for (const summary of summarize(rows)) {
    console.log(JSON.stringify({ type: "summary", ...summary }));
  }
}

await main();
