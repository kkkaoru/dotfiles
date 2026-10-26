import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
  CLAUDEX_PROVIDER_ID,
  CLAUDEX_WORKING_DIRECTORY_HEADER,
  createClaudexProviderConfig,
  withoutSystemPrompt,
  workingDirectoryHeader,
} from "./src/claudex-provider.ts";
import { resolveGatewayConfig } from "./src/config.ts";
import { startGatewayServer, type GatewayServer } from "./src/socket-server.ts";

export default async function claudexExtension(pi: ExtensionAPI): Promise<void> {
  pi.registerProvider(CLAUDEX_PROVIDER_ID, await createClaudexProviderConfig());
  pi.on("before_provider_request", (event, context) =>
    context.model?.provider === CLAUDEX_PROVIDER_ID
      ? withoutSystemPrompt(event.payload)
      : undefined,
  );
  pi.on("before_provider_headers", (event, context) => {
    if (context.model?.provider === CLAUDEX_PROVIDER_ID) {
      event.headers[CLAUDEX_WORKING_DIRECTORY_HEADER] = workingDirectoryHeader(context.cwd);
    }
  });
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
