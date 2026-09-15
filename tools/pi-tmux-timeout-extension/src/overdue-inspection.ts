// This TypeScript file is executed with Bun.
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function inspectedLogPath(event: unknown): string | undefined {
  if (
    !isRecord(event) ||
    event["toolName"] !== "read" ||
    event["isError"] !== false ||
    !isRecord(event["input"]) ||
    typeof event["input"]["path"] !== "string"
  ) {
    return undefined;
  }
  return event["input"]["path"].replace(/^@/u, "");
}
