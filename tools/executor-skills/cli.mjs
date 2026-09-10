// Runs with Bun. Keep stdout reserved for MCP JSON-RPC.
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createServer } from "./server.ts";

await createServer(process.argv.slice(2)).connect(new StdioServerTransport());
