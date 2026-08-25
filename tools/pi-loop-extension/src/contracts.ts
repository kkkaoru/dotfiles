// This TypeScript file is executed with Bun.

export interface LoopContext {
  readonly isIdle: () => boolean;
  readonly sessionManager?: { readonly getEntries: () => readonly unknown[] };
  readonly ui: {
    readonly notify: (message: string, level?: "error" | "info" | "warning") => void;
    readonly setStatus: (key: string, value: string | undefined) => void;
    readonly setWidget?: (key: string, lines: readonly string[] | undefined) => void;
  };
}

export interface LoopHost {
  readonly appendEntry?: (customType: string, data: unknown) => void;
  readonly sendUserMessage: (content: string) => void;
}
