export class GatewayError extends Error {
  readonly requestId: string | undefined;
  readonly fatal: boolean;

  constructor(message: string, requestId?: string, fatal = false) {
    super(message);
    this.name = "GatewayError";
    this.requestId = requestId;
    this.fatal = fatal;
  }
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
