#!/usr/bin/env bun
// Runs with Bun. Only the configured official agmsg scripts are executable.
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createAgmsgServer, runScript } from "./agmsg-server.ts";

const scriptsDirectory = process.argv[2];
if (!scriptsDirectory)
  throw new Error("Provide the trusted agmsg scripts directory");
await createAgmsgServer({ scriptsDirectory, run: runScript }).connect(
  new StdioServerTransport(),
);
