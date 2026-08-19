import type {
  Api,
  AssistantMessageEvent,
  Model,
  ProviderHeaders,
  SimpleStreamOptions,
} from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { toPiContext } from "./context-converter.ts";
import { GatewayError } from "./errors.ts";
import type { StreamRequestMessage } from "./protocol.ts";

type ModelRegistry = ExtensionContext["modelRegistry"];

interface StreamAuth {
  apiKey?: string;
  headers?: ProviderHeaders;
  env?: Record<string, string>;
}

export interface DirectStreamInput {
  request: StreamRequestMessage;
  registry: ModelRegistry;
  signal: AbortSignal;
  onEvent: (event: AssistantMessageEvent) => Promise<void>;
}

interface StreamReadState {
  terminal: boolean;
}

const CANCELLED_MESSAGE = "Cancelled by Claudex";

function resolveAvailableModel(registry: ModelRegistry, request: StreamRequestMessage): Model<Api> {
  let model: Model<Api> | null = null;
  for (const candidate of registry.getAll()) {
    if (candidate.provider === request.provider && candidate.id === request.modelId) {
      model = candidate;
      break;
    }
  }
  const available = registry
    .getAvailable()
    .some(
      (candidate) => candidate.provider === request.provider && candidate.id === request.modelId,
    );
  if (model === null || !available) {
    throw new GatewayError(
      `Pi model is unavailable: ${request.provider}/${request.modelId}`,
      request.id,
    );
  }
  return model;
}

function applyAuthOptions(options: SimpleStreamOptions, auth: StreamAuth): void {
  if (auth.apiKey !== undefined) {
    options.apiKey = auth.apiKey;
  }
  if (auth.headers !== undefined) {
    options.headers = auth.headers;
  }
  if (auth.env !== undefined) {
    options.env = auth.env;
  }
}

function applyRequestOptions(options: SimpleStreamOptions, request: StreamRequestMessage): void {
  const requestOptions = request.options;
  if (requestOptions.reasoning !== undefined && requestOptions.reasoning !== "off") {
    options.reasoning = requestOptions.reasoning;
  }
  if (requestOptions.maxTokens !== undefined) {
    options.maxTokens = requestOptions.maxTokens;
  }
  if (requestOptions.temperature !== undefined) {
    options.temperature = requestOptions.temperature;
  }
  if (requestOptions.samplingParams !== undefined) {
    options.samplingParams = requestOptions.samplingParams;
  }
  if (requestOptions.metadata !== undefined) {
    options.metadata = requestOptions.metadata;
  }
  if (requestOptions.sessionId !== undefined) {
    options.sessionId = requestOptions.sessionId;
  }
  if (requestOptions.cacheRetention !== undefined) {
    options.cacheRetention = requestOptions.cacheRetention;
  }
}

function buildOptions(
  request: StreamRequestMessage,
  signal: AbortSignal,
  auth: StreamAuth,
): SimpleStreamOptions {
  const options: SimpleStreamOptions = { signal };
  applyAuthOptions(options, auth);
  applyRequestOptions(options, request);
  return options;
}

function throwIfAborted(signal: AbortSignal, requestId: string): void {
  if (signal.aborted) {
    throw new GatewayError(CANCELLED_MESSAGE, requestId);
  }
}

async function abortError(signal: AbortSignal, requestId: string): Promise<never> {
  await new Promise<never>((_resolve, rejectError) => {
    const rejectCancelled = (): void => {
      rejectError(new GatewayError(CANCELLED_MESSAGE, requestId));
    };
    if (signal.aborted) {
      rejectCancelled();
      return;
    }
    signal.addEventListener("abort", rejectCancelled, { once: true });
  });
  throw new GatewayError(CANCELLED_MESSAGE, requestId);
}

async function nextEvent(
  iterator: AsyncIterator<AssistantMessageEvent>,
  signal: AbortSignal,
  requestId: string,
): Promise<IteratorResult<AssistantMessageEvent>> {
  throwIfAborted(signal, requestId);
  return Promise.race([iterator.next(), abortError(signal, requestId)]);
}

function abortableEvents(
  stream: AsyncIterable<AssistantMessageEvent>,
  signal: AbortSignal,
  requestId: string,
): AsyncIterable<AssistantMessageEvent> {
  return {
    [Symbol.asyncIterator](): AsyncIterator<AssistantMessageEvent> {
      const iterator = stream[Symbol.asyncIterator]();
      return {
        next: async (): Promise<IteratorResult<AssistantMessageEvent>> =>
          nextEvent(iterator, signal, requestId),
        return: async (): Promise<IteratorResult<AssistantMessageEvent>> => {
          if (iterator.return === undefined) {
            return { done: true, value: undefined };
          }
          return iterator.return();
        },
      };
    },
  };
}

export async function streamDirectModel(input: DirectStreamInput): Promise<void> {
  const { request, registry, signal, onEvent } = input;
  const model = resolveAvailableModel(registry, request);
  const provider = registry.getProvider(request.provider);
  if (provider === undefined) {
    throw new GatewayError(`Pi provider not found: ${request.provider}`, request.id);
  }
  const auth = await registry.getApiKeyAndHeaders(model);
  if (!auth.ok) {
    throw new GatewayError(`Pi provider authentication failed: ${auth.error}`, request.id);
  }
  const resolvedModel = auth.baseUrl === undefined ? model : { ...model, baseUrl: auth.baseUrl };
  const context = toPiContext(request, resolvedModel);
  const options = buildOptions(request, signal, auth);
  const state: StreamReadState = { terminal: false };
  const stream = abortableEvents(
    provider.streamSimple(resolvedModel, context, options),
    signal,
    request.id,
  );
  for await (const event of stream) {
    await onEvent(event);
    if (event.type === "done" || event.type === "error") {
      state.terminal = true;
    }
    throwIfAborted(signal, request.id);
  }
  if (!state.terminal) {
    throw new GatewayError("Pi provider stream ended without a terminal event", request.id);
  }
}
