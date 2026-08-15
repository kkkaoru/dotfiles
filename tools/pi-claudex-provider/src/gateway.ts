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
} from "./protocol.ts";

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

  private async writeProtocolError(id: string, message: string): Promise<void> {
    await this.writer.write(serverMessage("protocol_error", { id, message }));
  }
}
