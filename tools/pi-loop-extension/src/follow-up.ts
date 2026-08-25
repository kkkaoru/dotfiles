// This TypeScript file is executed with Bun.

const MAX_FOLLOW_UP_IDENTITY_CHARACTERS = 120;

export interface NamedLoopFollowUpInput {
  readonly completedAt: number;
  readonly identity: string;
  readonly prompt: string;
  readonly submittedAt: number;
}

function twoDigits(value: number): string {
  return String(value).padStart(2, "0");
}

function followUpTimestamp(timestamp: number, includeDate: boolean): string {
  const date = new Date(timestamp);
  const time = `${twoDigits(date.getHours())}:${twoDigits(date.getMinutes())}`;
  return includeDate
    ? `${twoDigits(date.getMonth() + 1)}-${twoDigits(date.getDate())} ${time}`
    : time;
}

function followUpIdentity(value: string): string {
  return value.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_FOLLOW_UP_IDENTITY_CHARACTERS);
}

export function namedLoopFollowUp(input: NamedLoopFollowUpInput): string {
  return `${followUpTimestamp(input.submittedAt, true)} → ${followUpTimestamp(input.completedAt, false)} | loop=${followUpIdentity(input.identity)}\n${input.prompt}`;
}

export function namedLoopSchedule(input: {
  readonly identity: string;
  readonly scheduledAt: number;
  readonly submittedAt: number;
}): string {
  return `${followUpTimestamp(input.submittedAt, true)} → ${followUpTimestamp(input.scheduledAt, false)} | loop=${followUpIdentity(input.identity)}`;
}
