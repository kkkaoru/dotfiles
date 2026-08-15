import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createClaudexProviderConfig, CLAUDEX_PROVIDER_ID } from "./src/claudex-provider.ts";
import { resolveGatewayConfig } from "./src/config.ts";
import { startGatewayServer, type GatewayServer } from "./src/socket-server.ts";

export default async function claudexExtension(pi: ExtensionAPI): Promise<void> {
  pi.registerProvider(CLAUDEX_PROVIDER_ID, await createClaudexProviderConfig());
  const config = resolveGatewayConfig();
  if (config === undefined) {
    return;
  }
  let server: GatewayServer | undefined = undefined;
  pi.on("session_start", async (_event, context) => {
    await server?.close();
    server = await startGatewayServer(config, context.modelRegistry);
  });
  pi.on("session_shutdown", async () => {
    const current = server;
    server = undefined;
    await current?.close();
  });
}
