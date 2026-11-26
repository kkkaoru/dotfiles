// This file runs with bun.
import type {
  ImageContent,
  JsonObject,
  Message,
  TextContent,
  Tool,
  TranscriptContext,
} from "@earendil-works/pi-ai";

import { isRecord } from "./json.ts";
import { replayTranscript } from "./transcript.ts";

/** One entry of the Interactions API `input` array. */
export type Step = { type: string } & Record<string, unknown>;

/** A declared tool, in the shape `POST /v1beta/interactions` accepts. */
export interface FunctionTool {
  type: "function";
  name: string;
  description?: string;
  parameters?: JsonObject;
}

export interface InteractionBody {
  input: Step[];
  model: string;
  stream: boolean;
  tools?: FunctionTool[];
}

export interface BodyOptions {
  model: string;
  messages: TranscriptContext["messages"];
}

type Blocks = readonly (TextContent | ImageContent)[];

function contentSteps(content: string | Blocks): Step[] {
  if (typeof content === "string") {
    return [{ type: "text", text: content }];
  }
  const steps: Step[] = [];
  for (const block of content) {
    if (block.type === "text") {
      steps.push({ type: "text", text: block.text });
    } else if (block.type === "image") {
      steps.push({ type: "image", data: block.data, mime_type: block.mimeType });
    }
  }
  return steps;
}

function functionTool(tool: Tool): FunctionTool {
  const declared: FunctionTool = { type: "function", name: tool.name };
  if (tool.description) {
    declared.description = tool.description;
  }
  if (isRecord(tool.parameters)) {
    declared.parameters = tool.parameters;
  }
  return declared;
}

function assistantStep(message: Extract<Message, { role: "assistant" }>): Step | undefined {
  const content: Step[] = [];
  for (const block of message.content) {
    if (block.type === "text") {
      content.push({ type: "text", text: block.text });
      continue;
    }
    if (block.type === "toolCall") {
      content.push({
        type: "function_call",
        id: block.id,
        name: block.name,
        arguments: block.arguments,
      });
    }
  }
  return content.length === 0 ? undefined : { type: "model_output", content };
}

function toolResultStep(message: Extract<Message, { role: "toolResult" }>): Step {
  const result = message.content
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("\n");
  const step: Step = {
    type: "function_result",
    call_id: message.toolCallId,
    name: message.toolName,
    result,
  };
  if (message.isError) {
    step["is_error"] = true;
  }
  return step;
}

/** System messages carry the prompt and the tool declarations, so they never become steps. */
function messageStep(message: Message): Step | undefined {
  switch (message.role) {
    case "user":
      return { type: "user_input", content: contentSteps(message.content) };
    case "assistant":
      return assistantStep(message);
    case "toolResult":
      return toolResultStep(message);
    default:
      return undefined;
  }
}

/** Build the request body for one completion from pi's transcript. */
export function buildBody({ model, messages }: BodyOptions): InteractionBody {
  const { systemPrompt, tools } = replayTranscript(messages);
  const input: Step[] = [];
  if (systemPrompt.trim().length > 0) {
    input.push({ type: "user_input", content: [{ type: "text", text: systemPrompt }] });
  }
  for (const message of messages) {
    const step = messageStep(message);
    if (step) {
      input.push(step);
    }
  }
  const body: InteractionBody = { input, model, stream: false };
  if (tools.length > 0) {
    body.tools = tools.map(functionTool);
  }
  return body;
}
