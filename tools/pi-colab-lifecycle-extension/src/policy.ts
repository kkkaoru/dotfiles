export const PROVIDER = "google-colab";
export const SESSION = "qwen";
export const IDLE_MS = 10 * 60 * 1000;

export interface IdleInput {
  readonly now: number;
  readonly lastRequestAt: number;
  readonly running: boolean;
}

/** Colab is stopped only while running and after 10 minutes without a request. */
export function shouldStop(input: IdleInput, idleMs: number = IDLE_MS): boolean {
  return input.running && input.now - input.lastRequestAt >= idleMs;
}

export function isColabModel(model: { readonly provider: string } | undefined): boolean {
  return model?.provider === PROVIDER;
}

export interface UsageInfo {
  readonly balance: number;
  readonly ratePerHour: number;
}

const BALANCE_PATTERN = /Current balance:\s*([\d.]+)/u;
const RATE_PATTERN = /Usage rate:\s*([\d.]+)\/hr/u;

/** Parse `colab usage` output ("Current balance: 194.09 compute units", "Usage rate: 5.38/hr"). */
export function parseUsage(stdout: string): UsageInfo | undefined {
  const balance = BALANCE_PATTERN.exec(stdout)?.[1];
  const rate = RATE_PATTERN.exec(stdout)?.[1];
  return balance === undefined || rate === undefined
    ? undefined
    : { balance: Number(balance), ratePerHour: Number(rate) };
}

export function formatUsage(info: UsageInfo): string {
  const hours =
    info.ratePerHour > 0 ? ` · ~${(info.balance / info.ratePerHour).toFixed(1)}h left` : "";
  return `Colab ${info.balance.toFixed(1)} CU · ${info.ratePerHour}/hr${hours}`;
}
