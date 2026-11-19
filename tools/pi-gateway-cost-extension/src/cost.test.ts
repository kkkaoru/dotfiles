// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import {
  accountIdFrom,
  configOf,
  formatCost,
  parseCost,
  queryBody,
  windowStart,
  tokenFrom,
  wranglerToken,
} from "./cost.ts";

const config = { accountId: "acct", gateway: "pi-agent" };
const section = { gateway: "pi-agent" };

test("the query names the gateway and the window", () => {
  const body = JSON.parse(
    queryBody(config, "2026-11-18T00:00:00.000Z", "2026-11-18T12:00:00.000Z"),
  ) as {
    query: string;
    variables: Record<string, string>;
  };
  expect(body.query).toContain("aiGatewayRequestsAdaptiveGroups");
  expect(body.query).toContain("sum { cost }");
  expect(body.variables).toStrictEqual({
    accountTag: "acct",
    end: "2026-11-18T12:00:00.000Z",
    gateway: "pi-agent",
    start: "2026-11-18T00:00:00.000Z",
  });
});

test("the cost is read from the first group", () => {
  expect(
    parseCost({
      data: {
        viewer: { accounts: [{ aiGatewayRequestsAdaptiveGroups: [{ sum: { cost: 16.7128 } }] }] },
      },
    }),
  ).toBeCloseTo(16.7128);
  expect(
    parseCost({ data: { viewer: { accounts: [{ aiGatewayRequestsAdaptiveGroups: [] }] } } }),
  ).toBeUndefined();
  expect(parseCost({})).toBeUndefined();
  expect(
    parseCost({
      data: {
        viewer: { accounts: [{ aiGatewayRequestsAdaptiveGroups: [{ sum: { cost: "x" } }] }] },
      },
    }),
  ).toBeUndefined();
});

test("the footer text carries two decimals", () => {
  expect(formatCost(16.7128)).toBe("gateway $16.7/24h");
  expect(formatCost(0)).toBe("gateway $0.0/24h");
});

test("the window is the rolling last 24 hours", () => {
  const start = windowStart(new Date(2026, 10, 18, 15, 30));
  expect(start).toBe(new Date(2026, 10, 17, 15, 30).toISOString());
});

test("the settings section only names the gateway", () => {
  expect(configOf(JSON.stringify({ "pi-gateway-cost": section }))).toStrictEqual(section);
  expect(configOf(JSON.stringify({ "pi-gateway-cost": { accountId: "acct" } }))).toBeUndefined();
  expect(configOf(JSON.stringify({ "pi-gateway-cost": "pi-agent" }))).toBeUndefined();
  expect(configOf("{}")).toBeUndefined();
});

test("the account id comes from the environment, else the decrypted secret", () => {
  expect(accountIdFrom(" env-acct ", undefined)).toBe("env-acct");
  expect(accountIdFrom("", '{"accountId":"secret-acct"}')).toBe("secret-acct");
  expect(accountIdFrom(undefined, '{"accountId":"secret-acct"}')).toBe("secret-acct");
  expect(accountIdFrom("", undefined)).toBeUndefined();
  expect(accountIdFrom("", "not json")).toBeUndefined();
  expect(accountIdFrom("", '{"accountId":1}')).toBeUndefined();
});

test("an environment token wins over the secret and wrangler's", () => {
  const wrangler = 'oauth_token = "from-wrangler"\n';
  const secret = '{"accountId":"acct","token":" from-secret "}';
  expect(tokenFrom(" from-env ", wrangler, secret)).toBe("from-env");
  expect(tokenFrom("", wrangler, secret)).toBe("from-secret");
  expect(tokenFrom(undefined, wrangler, '{"accountId":"acct"}')).toBe("from-wrangler");
  expect(tokenFrom(undefined, wrangler, "not json")).toBe("from-wrangler");
  expect(tokenFrom("", wrangler)).toBe("from-wrangler");
  expect(tokenFrom(undefined, wrangler)).toBe("from-wrangler");
  expect(tokenFrom("  ", undefined)).toBeUndefined();
  expect(wranglerToken('expiration_time = "2026"')).toBeUndefined();
});
