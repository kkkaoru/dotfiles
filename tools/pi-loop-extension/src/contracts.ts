// This TypeScript file is executed with Bun.

export interface CompleteResult {
  readonly reason: string;
}

export interface LoopContext {
  readonly isIdle: () => boolean;
  readonly sessionManager?: { readonly getEntries: () => readonly unknown[] };
  readonly ui: {
    readonly notify: (message: string, level?: "error" | "info" | "warning") => void;
    readonly setStatus: (key: string, value: string | undefined) => void;
    readonly setWidget?: (key: string, lines: readonly string[] | undefined) => void;
  };
}

export interface UserMessageDeliveryOptions {
  readonly deliverAs: "followUp";
}

export interface LoopHost {
  readonly appendEntry?: (customType: string, data: unknown) => void;
  readonly sendUserMessage: (content: string, options?: UserMessageDeliveryOptions) => void;
}
