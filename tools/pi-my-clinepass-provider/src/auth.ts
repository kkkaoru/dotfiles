import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { isRecord } from "./utils.ts";

const CLINE_PROVIDER_KEYS = ["cline-pass", "cline"] as const;

export function defaultAuthPaths(home: string): string[] {
  return [
    path.join(home, ".cline", "data", "settings", "providers.json"),
    path.join(home, ".pi", "agent", "auth.json"),
  ];
}

export interface AuthKeyOptions {
  authPaths?: readonly string[];
  fileExists?: (filePath: string) => boolean;
  homeDir?: () => string;
  readFile?: (filePath: string) => string;
}

function parseAuthFile(
  authPath: string,
  readFile: (filePath: string) => string,
  fileExists: (filePath: string) => boolean,
): Record<string, unknown> | undefined {
  if (!fileExists(authPath)) {
    return undefined;
  }
  try {
    const parsed: unknown = JSON.parse(readFile(authPath));
    return isRecord(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

export function readAuthRecords(options: AuthKeyOptions = {}): Record<string, unknown>[] {
  const home = options.homeDir?.() ?? homedir();
  const authPaths = options.authPaths ?? defaultAuthPaths(home);
  const readFile =
    options.readFile ?? ((filePath: string): string => readFileSync(filePath, "utf8"));
  const fileExists = options.fileExists ?? ((filePath: string): boolean => existsSync(filePath));
  return authPaths.flatMap((authPath) => {
    const parsed = parseAuthFile(authPath, readFile, fileExists);
    return parsed === undefined ? [] : [parsed];
  });
}

export function walkAuthPaths<Value>(
  options: AuthKeyOptions,
  extract: (parsed: Record<string, unknown>) => Value | undefined,
): Value | undefined {
  for (const parsed of readAuthRecords(options)) {
    const result = extract(parsed);
    if (result !== undefined) {
      return result;
    }
  }
  return undefined;
}

export function walkClineProviderSettings<Value>(
  parsed: Record<string, unknown>,
  extract: (settings: Record<string, unknown>) => Value | undefined,
): Value | undefined {
  const providers = isRecord(parsed["providers"]) ? parsed["providers"] : undefined;
  if (providers === undefined) {
    return undefined;
  }

  for (const providerKey of CLINE_PROVIDER_KEYS) {
    const provider = isRecord(providers[providerKey]) ? providers[providerKey] : undefined;
    const settings = isRecord(provider?.["settings"]) ? provider["settings"] : undefined;
    if (settings !== undefined) {
      const result = extract(settings);
      if (result !== undefined) {
        return result;
      }
    }
  }
  return undefined;
}
