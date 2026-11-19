// This TypeScript file is executed with Bun.

export const QUERY = [
  "query ($accountTag: string!, $start: Time!, $end: Time!, $gateway: string!) {",
  "  viewer {",
  "    accounts(filter: { accountTag: $accountTag }) {",
  "      aiGatewayRequestsAdaptiveGroups(",
  "        limit: 1",
  "        filter: { gateway: $gateway, datetime_geq: $start, datetime_leq: $end }",
  "      ) {",
  "        count",
  "        sum { cost }",
  "      }",
  "    }",
  "  }",
  "}",
].join("\n");

export interface GatewayConfig {
  readonly gateway: string;
}

export interface GatewayQuery extends GatewayConfig {
  readonly accountId: string;
}

/** The `oauth_token` wrangler stores, so a machine that already runs wrangler needs no extra token. */
export function wranglerToken(config: string): string | undefined {
  return /^\s*oauth_token\s*=\s*"([^"]+)"/mu.exec(config)?.[1];
}

/** The fields of the encrypted gateway-cost secret, or none when it is not JSON. */
function secretOf(secret: string | undefined): Record<string, unknown> {
  if (secret === undefined) {
    return {};
  }
  try {
    return JSON.parse(secret) as Record<string, unknown>;
  } catch {
    return {};
  }
}

/**
 * A token from the environment wins, then a scoped token kept in the secret (it does not expire),
 * then wrangler's own OAuth token, which does.
 */
export function tokenFrom(
  envToken: string | undefined,
  wranglerConfig: string | undefined,
  secret?: string,
): string | undefined {
  const fromEnv = envToken?.trim();
  if (fromEnv !== undefined && fromEnv !== "") {
    return fromEnv;
  }
  const { token } = secretOf(secret);
  if (typeof token === "string" && token.trim() !== "") {
    return token.trim();
  }
  return wranglerConfig === undefined ? undefined : wranglerToken(wranglerConfig);
}

/** The `pi-gateway-cost` section of settings.json, or undefined when the machine does not set one. */
export function configOf(settings: string): GatewayConfig | undefined {
  const section = (JSON.parse(settings) as Record<string, unknown>)["pi-gateway-cost"];
  if (typeof section !== "object" || section === null) {
    return undefined;
  }
  const { gateway } = section as Record<string, unknown>;
  return typeof gateway === "string" ? { gateway } : undefined;
}

/** The account id from the environment, else from the decrypted secret, else nothing. */
export function accountIdFrom(
  envToken: string | undefined,
  secret: string | undefined,
): string | undefined {
  const fromEnv = envToken?.trim();
  if (fromEnv !== undefined && fromEnv !== "") {
    return fromEnv;
  }
  const { accountId } = secretOf(secret);
  return typeof accountId === "string" ? accountId : undefined;
}

export function queryBody(config: GatewayQuery, start: string, end: string): string {
  return JSON.stringify({
    query: QUERY,
    variables: {
      accountTag: config.accountId,
      end,
      gateway: config.gateway,
      start,
    },
  });
}

interface Group {
  readonly sum?: { readonly cost?: unknown };
}

function firstGroup(response: unknown): Group | undefined {
  const data = (response as { data?: { viewer?: { accounts?: readonly unknown[] } } } | null)?.data;
  const account = data?.viewer?.accounts?.[0] as
    | { aiGatewayRequestsAdaptiveGroups?: readonly unknown[] }
    | undefined;
  return account?.aiGatewayRequestsAdaptiveGroups?.[0] as Group | undefined;
}

/** The gateway's estimated spend in USD for the window, or undefined when the answer has none. */
export function parseCost(response: unknown): number | undefined {
  const cost = firstGroup(response)?.sum?.cost;
  return typeof cost === "number" && Number.isFinite(cost) ? cost : undefined;
}

export function formatCost(cost: number): string {
  return `gateway $${cost.toFixed(1)}/24h`;
}

/** Midnight before `now` in local time, as the GraphQL API wants it. */
/** The window the AI Gateway dashboard calls "Last 24 hours", which `scripts/gateway-cost.sh` asks
 *  for too, so both displays agree. */
export function windowStart(now: Date): string {
  return new Date(now.getTime() - 24 * 60 * 60 * 1000).toISOString();
}
