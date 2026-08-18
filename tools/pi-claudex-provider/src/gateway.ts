import type { Api, Model, TextContent } from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { streamDirectModel } from "./direct-stream.ts";
import { errorMessage, GatewayError } from "./errors.ts";
import { mapAssistantEvent } from "./event-mapper.ts";
import {
  serverMessage,
  type ClientMessage,
  type ListModelsMessage,
  type ServerMessage,
  type StreamRequestMessage,
  type WebSearchRequest,
} from "./protocol.ts";
import { parseSearchTriplets } from "./search-parser.ts";

type ModelRegistry = ExtensionContext["modelRegistry"];

export interface GatewayWriter {
  write: (message: ServerMessage) => Promise<void>;
}

function observe(promise: Promise<unknown>): void {
  promise.catch(() => null);
}

export class GatewayConnection {
  private readonly registry: ModelRegistry;
  private readonly writer: GatewayWriter;
  private readonly active = new Map<string, AbortController>();
  private closed = false;

  constructor(registry: ModelRegistry, writer: GatewayWriter) {
    this.registry = registry;
    this.writer = writer;
  }

  handle(message: Exclude<ClientMessage, { type: "hello" }>): void {
    if (this.closed) {
      return;
    }
    if (message.type === "cancel") {
      this.cancel(message.id);
      return;
    }
    if (message.type === "list_models") {
      observe(this.listModels(message));
      return;
    }
    if (message.type === "web_search") {
      observe(this.webSearch(message));
      return;
    }
    this.startRequest(message);
  }

  close(): void {
    this.closed = true;
    for (const controller of this.active.values()) {
      controller.abort("Pi gateway connection closed");
    }
    this.active.clear();
  }

  private startRequest(request: StreamRequestMessage): void {
    if (this.active.has(request.id)) {
      observe(this.writeProtocolError(request.id, `Duplicate active request id: ${request.id}`));
      return;
    }
    const controller = new AbortController();
    this.active.set(request.id, controller);
    const operation = this.runRequest(request, controller).finally(() => {
      this.active.delete(request.id);
    });
    observe(operation);
  }

  private async runRequest(
    request: StreamRequestMessage,
    controller: AbortController,
  ): Promise<void> {
    let terminalSent = false;
    try {
      await streamDirectModel({
        request,
        registry: this.registry,
        signal: controller.signal,
        onEvent: async (event) => {
          if (terminalSent) {
            throw new GatewayError("Pi provider emitted an event after termination", request.id);
          }
          await this.writer.write(mapAssistantEvent(request.id, event));
          terminalSent = event.type === "done" || event.type === "error";
        },
      });
    } catch (error) {
      if (!terminalSent) {
        await this.writeProtocolError(request.id, errorMessage(error));
      }
    }
  }

  private cancel(id: string): void {
    const controller = this.active.get(id);
    if (controller === undefined) {
      observe(this.writeProtocolError(id, `No active request for cancellation: ${id}`));
      return;
    }
    controller.abort("Cancelled by Claudex");
  }

  private async listModels(message: ListModelsMessage): Promise<void> {
    const models = this.registry
      .getAvailable()
      .filter((model) => model.provider !== "claudex")
      .map((model) => ({
        provider: model.provider,
        id: model.id,
        name: model.name,
        api: model.api,
        reasoning: model.reasoning,
        input: model.input,
        contextWindow: model.contextWindow,
        maxTokens: model.maxTokens,
      }));
    await this.writer.write(serverMessage("models", { id: message.id, models }));
  }

  private findModel(provider: string, modelId: string): Model<Api> | undefined {
    // eslint-disable-next-line unicorn/no-array-method-this-argument -- ModelRegistry.find
    const exact = this.registry.find(provider, modelId);
    if (exact !== undefined) {
      return exact;
    }
    // Provider extensions may register models under a different provider name.
    // Fall back to a provider-agnostic lookup so that delegate-pi web searches use Exa.
    return this.registry.getAvailable().find((model) => model.id === modelId);
  }

  private async webSearch(request: WebSearchRequest): Promise<void> {
    const model = this.findModel(request.provider, request.modelId);
    if (model?.provider === "cursor") {
      await this.cursorWebSearch(request, model);
      return;
    }
    // For all other providers, use Exa directly with the requested piProvider/modelId.
    // This avoids a PiGateway fallback when the model is not yet in the registry.
    await this.exaWebSearch(request, { provider: request.provider, id: request.modelId });
  }

  private async sendWebSearchError(request: WebSearchRequest, message: string): Promise<void> {
    const model = this.findModel(request.provider, request.modelId);
    await this.writer.write(
      serverMessage("web_search_error", {
        id: request.id,
        provider: request.provider,
        modelId: request.modelId,
        message,
        modelProvider: model?.provider,
      }),
    );
  }

  private async cursorWebSearch(request: WebSearchRequest, model: Model<Api>): Promise<void> {
    if (!this.registry.hasConfiguredAuth(model)) {
      await this.sendWebSearchError(
        request,
        `Authentication not configured for ${model.provider}/${model.id}`,
      );
      return;
    }
    try {
      const maxTokens = request.options?.maxTokens;
      const temperature = request.options?.temperature;
      const samplingParams = request.options?.samplingParams;
      const result = await this.registry.complete(
        model,
        {
          systemPrompt: "You are a helpful assistant with live web search capability.",
          messages: [
            {
              role: "user" as const,
              content: `Search the web for "${request.query}". Return ONLY raw search results as Title:/URL:/Snippet: triplets. Include exactly 5 results. Do NOT add any interpretation, summary, or commentary.`,
              timestamp: Date.now(),
            },
          ],
          tools: [],
        },
        {
          signal: AbortSignal.timeout(30_000),
          ...(maxTokens !== undefined && { maxTokens }),
          ...(temperature !== undefined && { temperature }),
          ...(samplingParams !== undefined && { samplingParams }),
        },
      );
      const fullText = result.content
        .filter((block): block is TextContent => block.type === "text")
        .map((block) => block.text)
        .join("\n");
      const parsed = parseSearchTriplets(fullText);
      await this.writer.write(
        serverMessage("web_search_result", {
          id: request.id,
          provider: model.provider,
          modelId: model.id,
          results: parsed,
        }),
      );
    } catch (error: unknown) {
      await this.sendWebSearchError(request, errorMessage(error));
    }
  }

  private async exaWebSearch(
    request: WebSearchRequest,
    model: { provider: string; id: string },
  ): Promise<void> {
    const apiKey = process.env["EXA_API_KEY"];
    if (apiKey === undefined || apiKey === "") {
      await this.sendWebSearchError(
        request,
        "EXA_API_KEY environment variable is not set. Configure it to enable web search for non-Cursor providers.",
      );
      return;
    }
    try {
      const response = await fetch("https://api.exa.ai/search", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${apiKey}`,
        },
        body: JSON.stringify({
          query: request.query,
          numResults: 5,
          contents: { text: true },
        }),
        signal: AbortSignal.timeout(30_000),
      });
      if (!response.ok) {
        const body = await response.text();
        await this.sendWebSearchError(
          request,
          `Exa API error (${response.status}): ${body.slice(0, 200)}`,
        );
        return;
      }
      const data = (await response.json()) as {
        results?: { title?: string; url?: string; text?: string }[];
      };
      const results = (data.results ?? []).map((entry) => ({
        title: entry.title ?? "",
        url: entry.url ?? "",
        snippet: (entry.text ?? "").slice(0, 300),
      }));
      await this.writer.write(
        serverMessage("web_search_result", {
          id: request.id,
          provider: model.provider,
          modelId: model.id,
          results,
        }),
      );
    } catch (error: unknown) {
      await this.sendWebSearchError(request, `Exa search failed: ${errorMessage(error)}`);
    }
  }

  private async writeProtocolError(id: string, message: string): Promise<void> {
    await this.writer.write(serverMessage("protocol_error", { id, message }));
  }
}
