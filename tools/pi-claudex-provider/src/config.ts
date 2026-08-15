import path from "node:path";

export const GATEWAY_SOCKET_ENV = "CLAUDEX_PI_GATEWAY_SOCKET";
export const GATEWAY_TOKEN_ENV = "CLAUDEX_PI_GATEWAY_TOKEN";
const MAX_UNIX_SOCKET_PATH_BYTES = 103;
const MIN_TOKEN_LENGTH = 32;

export interface GatewayConfig {
  socketPath: string;
  token: string;
}

function validateConfig(socketPath: string, token: string): GatewayConfig {
  if (!path.isAbsolute(socketPath)) {
    throw new Error(`${GATEWAY_SOCKET_ENV} must be an absolute path`);
  }
  if (Buffer.byteLength(socketPath) > MAX_UNIX_SOCKET_PATH_BYTES) {
    throw new Error(`${GATEWAY_SOCKET_ENV} exceeds the Unix socket path limit`);
  }
  if (token.length < MIN_TOKEN_LENGTH) {
    throw new Error(`${GATEWAY_TOKEN_ENV} must contain at least ${MIN_TOKEN_LENGTH} characters`);
  }
  return { socketPath, token };
}

export function resolveGatewayConfig(
  env: Readonly<Record<string, string | undefined>> = process.env,
): GatewayConfig | undefined {
  const socketPath = env[GATEWAY_SOCKET_ENV]?.trim();
  const token = env[GATEWAY_TOKEN_ENV]?.trim();
  if (socketPath === undefined && token === undefined) {
    return undefined;
  }
  if (socketPath === undefined || socketPath === "" || token === undefined || token === "") {
    throw new Error(`${GATEWAY_SOCKET_ENV} and ${GATEWAY_TOKEN_ENV} must be configured together`);
  }
  return validateConfig(socketPath, token);
}
