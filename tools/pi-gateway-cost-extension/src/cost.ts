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

/** An Account Analytics Read token from the environment wins; wrangler's own token is the fallback. */
export function tokenFrom(
  envToken: string | undefined,
  wranglerConfig: string | undefined,
): string | undefined {
  const fromEnv = envToken?.trim();
  if (fromEnv !== undefined && fromEnv !== "") {
    return fromEnv;
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
  if (secret === undefined) {
    return undefined;
  }
  try {
    const { accountId } = JSON.parse(secret) as Record<string, unknown>;
    return typeof accountId === "string" ? accountId : undefined;
  } catch {
    return undefined;
  }
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
  return `gateway $${cost.toFixed(2)} today`;
}

/** Midnight before `now` in local time, as the GraphQL API wants it. */
export function startOfDay(now: Date): string {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate()).toISOString();
}
