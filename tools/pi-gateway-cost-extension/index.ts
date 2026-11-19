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
  windowStart,
  tokenFrom,
  type GatewayConfig,
} from "./src/cost.ts";

const STATUS_KEY = "gateway-cost";
const REFRESH_MS = 60_000;
const ENDPOINT = "https://api.cloudflare.com/client/v4/graphql";
const RENEW = "renew-wrangler-token";

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

function token(secret: string | undefined): string | undefined {
  const wrangler = path.join(homedir(), ".wrangler", "config", "default.toml");
  return tokenFrom(process.env["CLOUDFLARE_API_TOKEN"], readText(wrangler), secret);
}

/** Wrangler's OAuth token expires hourly; wrangler renews the file it wrote when asked. */
function renewWranglerToken(): void {
  spawnSync("wrangler", ["whoami"], { stdio: "ignore" });
}

/** The repository checkout this extension lives in; its scripts own the age identity lookup. */
export function secretsScript(): string {
  return path.resolve(import.meta.dirname, "..", "..", "scripts", "secrets.sh");
}

// The account id is a secret: Git holds only secrets/gateway-cost.age, and scripts/secrets.sh
// Decrypts it with the identity from 1Password, the keychain or ~/.config/age/keys.txt.
function decryptedSecret(): string | undefined {
  const script = secretsScript();
  if (!existsSync(script)) {
    return undefined;
  }
  const result = spawnSync("bash", [script, "decrypt", "gateway-cost"], { encoding: "utf8" });
  return result.status === 0 ? result.stdout : undefined;
}

// The gateway's own estimate is the number the dashboard shows.
// Pi's footer prices the same tokens from the model catalog instead.
// An expired wrangler token answers 401, which asks the caller to renew and try once more.
async function costOf(
  config: GatewayConfig & { accountId: string },
  key: string,
  now: Date,
): Promise<string | undefined> {
  const response = await globalThis.fetch(ENDPOINT, {
    body: queryBody(config, windowStart(now), now.toISOString()),
    headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
    method: "POST",
  });
  if (response.status === 401) {
    return RENEW;
  }
  if (!response.ok) {
    return undefined;
  }
  const cost = parseCost(await response.json());
  return cost === undefined ? undefined : formatCost(cost);
}

/** Ask the gateway, renewing wrangler's expired token once when the answer is 401. */
async function costWithRenewal(
  config: GatewayConfig & { accountId: string },
  key: string,
  secret: string | undefined,
  now: Date,
): Promise<string | undefined> {
  const text = await costOf(config, key, now);
  if (text !== RENEW) {
    return text;
  }
  renewWranglerToken();
  const again = await costOf(config, token(secret) ?? key, now);
  return again === RENEW ? undefined : again;
}

export default function gatewayCost(pi: ExtensionAPI): void {
  const state = {
    account: undefined as string | undefined,
    known: false,
    secret: undefined as string | undefined,
  };
  const run = { asking: false, at: 0, warned: false };
  const accountId = (): string | undefined => {
    if (!state.known) {
      state.secret = decryptedSecret();
      state.account = accountIdFrom(process.env["CLOUDFLARE_ACCOUNT_ID"], state.secret);
      state.known = true;
    }
    return state.account;
  };
  const refresh = async (ui: CostHost): Promise<void> => {
    const now = Date.now();
    const config = settingsConfig();
    const account = accountId();
    const key = token(state.secret);
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
      const text = await costWithRenewal(
        { ...config, accountId: account },
        key,
        state.secret,
        new Date(now),
      );
      if (typeof text === "string") {
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
