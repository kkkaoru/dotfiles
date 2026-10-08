// This TypeScript file is executed with Bun.
export function eventToolCallId(event: unknown): string | undefined {
  if (
    typeof event !== "object" ||
    event === null ||
    !("toolCallId" in event) ||
    typeof event.toolCallId !== "string"
  ) {
    return undefined;
  }
  return event.toolCallId;
}

export function toolResultFailed(event: unknown): boolean {
  return (
    typeof event === "object" && event !== null && "isError" in event && event.isError === true
  );
}
