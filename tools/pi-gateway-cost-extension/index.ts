// This TypeScript file is executed with Bun.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import process from "node:process";
import { getAgentDir, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
  accountIdFrom,
  configOf,
  formatCost,
  parseCost,
  queryBody,
  startOfDay,
  tokenFrom,
  type GatewayConfig,
} from "./src/cost.ts";

const STATUS_KEY = "gateway-cost";
const REFRESH_MS = 60_000;
const ENDPOINT = "https://api.cloudflare.com/client/v4/graphql";

export interface CostHost {
  notify(message: string, type?: "info" | "warning" | "error"): void;
  setStatus(key: string, text: string | undefined): void;
}

function readText(file: string): string | undefined {
  return existsSync(file) ? readFileSync(file, "utf8") : undefined;
}

function settingsConfig(): GatewayConfig | undefined {
  const file = path.join(getAgentDir(), "settings.json");
  try {
    return configOf(readFileSync(file, "utf8"));
  } catch {
    // An unreadable settings file means this machine shows no gateway cost.
  }
}

function token(): string | undefined {
  const wrangler = path.join(homedir(), ".wrangler", "config", "default.toml");
  return tokenFrom(process.env["CLOUDFLARE_API_TOKEN"], readText(wrangler));
}

// The account id is a secret: Git holds only secrets/gateway-cost.age, read with the owner's key.
function decryptedSecret(): string | undefined {
  const file = path.join(getAgentDir(), "secrets", "gateway-cost.age");
  const identity = path.join(homedir(), ".config", "age", "keys.txt");
  if (!existsSync(file) || !existsSync(identity)) {
    return undefined;
  }
  const result = spawnSync("age", ["-d", "-i", identity, file], { encoding: "utf8" });
  return result.status === 0 ? result.stdout : undefined;
}

// The gateway's own estimate is the number the dashboard shows.
// Pi's footer prices the same tokens from the model catalog instead.
async function costOf(
  config: GatewayConfig & { accountId: string },
  key: string,
  now: Date,
): Promise<string | undefined> {
  const response = await globalThis.fetch(ENDPOINT, {
    body: queryBody(config, startOfDay(now), now.toISOString()),
    headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
    method: "POST",
  });
  if (!response.ok) {
    return undefined;
  }
  const cost = parseCost(await response.json());
  return cost === undefined ? undefined : formatCost(cost);
}

export default function gatewayCost(pi: ExtensionAPI): void {
  const state = { account: { known: false, value: undefined as string | undefined } };
  const run = { asking: false, at: 0, warned: false };
  const accountId = (): string | undefined => {
    if (!state.account.known) {
      state.account = {
        known: true,
        value: accountIdFrom(process.env["CLOUDFLARE_ACCOUNT_ID"], decryptedSecret()),
      };
    }
    return state.account.value;
  };
  const refresh = async (ui: CostHost): Promise<void> => {
    const now = Date.now();
    const config = settingsConfig();
    const account = accountId();
    const key = token();
    if (
      run.asking ||
      now - run.at < REFRESH_MS ||
      config === undefined ||
      account === undefined ||
      key === undefined
    ) {
      return;
    }
    run.asking = true;
    run.at = now;
    try {
      const text = await costOf({ ...config, accountId: account }, key, new Date(now));
      if (text !== undefined) {
        ui.setStatus(STATUS_KEY, text);
        return;
      }
      if (!run.warned) {
        run.warned = true;
        ui.notify(
          "gateway-cost: the AI Gateway cost could not be read; check the token's Account Analytics Read permission.",
          "warning",
        );
      }
    } catch {
      // A failed read leaves the last shown value alone; the next turn asks again.
    } finally {
      run.asking = false;
    }
  };
  pi.on("session_start", (_event, ctx) => {
    void refresh(ctx.ui);
  });
  pi.on("turn_end", (_event, ctx) => {
    void refresh(ctx.ui);
  });
}
