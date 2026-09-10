// Runs with Bun; Vitest mocks all filesystem-backed catalog operations.
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { afterEach, beforeEach, expect, it, type Mock, vi } from "vitest";
import type { ReferenceResult, Skill } from "./catalog.ts";
import { createServer } from "./server.ts";

interface CatalogMocks {
  listSkills: Mock<() => Promise<Skill[]>>;
  searchSkills: Mock<() => Skill[]>;
  selectSkill: Mock<() => Skill>;
  readText: Mock<() => Promise<string>>;
  readReference: Mock<() => Promise<ReferenceResult>>;
}
interface Connection {
  client: Client;
  server: McpServer;
}

const catalog: CatalogMocks = vi.hoisted(() => ({
  listSkills: vi.fn(),
  searchSkills: vi.fn(),
  selectSkill: vi.fn(),
  readText: vi.fn(),
  readReference: vi.fn(),
}));
const connections: Connection[] = [];
vi.mock("./catalog.ts", () => catalog);

beforeEach(() => {
  vi.resetAllMocks();
  catalog.listSkills.mockResolvedValue([
    {
      id: "cloudflare",
      description: "Workers",
      directory: "/skills/cloudflare",
    },
  ]);
  catalog.searchSkills.mockReturnValue([
    {
      id: "cloudflare",
      description: "Workers",
      directory: "/skills/cloudflare",
    },
  ]);
  catalog.selectSkill.mockReturnValue({
    id: "cloudflare",
    description: "Workers",
    directory: "/skills/cloudflare",
  });
  catalog.readText.mockResolvedValue("Instructions");
  catalog.readReference.mockResolvedValue({
    text: "Docs",
    totalCharacters: 4,
    nextOffset: null,
  });
});

afterEach(async () => {
  await Promise.all(
    connections.splice(0).map(async (connection: Connection) => {
      await connection.client.close();
      await connection.server.close();
    }),
  );
});

const connect = async (): Promise<Client> => {
  const server: McpServer = createServer(["/skills"]);
  const client: Client = new Client({ name: "test", version: "1" });
  const [clientTransport, serverTransport]: [
    InMemoryTransport,
    InMemoryTransport,
  ] = InMemoryTransport.createLinkedPair();
  connections.push({ client, server });
  await server.connect(serverTransport);
  await client.connect(clientTransport);
  return client;
};

it("requires explicitly trusted roots", () => {
  expect(() => createServer([])).toThrow("trusted skill directory");
});

it("exposes only three read-only tools", async () => {
  const client: Client = await connect();
  expect(
    (await client.listTools()).tools.map((tool) => ({
      name: tool.name,
      annotations: tool.annotations,
    })),
  ).toStrictEqual([
    {
      name: "search_skills",
      annotations: {
        readOnlyHint: true,
        destructiveHint: false,
        openWorldHint: false,
      },
    },
    {
      name: "read_skill",
      annotations: {
        readOnlyHint: true,
        destructiveHint: false,
        openWorldHint: false,
      },
    },
    {
      name: "read_reference",
      annotations: {
        readOnlyHint: true,
        destructiveHint: false,
        openWorldHint: false,
      },
    },
  ]);
});

it("serves a bounded skill search", async () => {
  const client: Client = await connect();
  expect(
    await client.callTool({
      name: "search_skills",
      arguments: { query: "workers", limit: 5 },
    }),
  ).toStrictEqual({
    content: [
      {
        type: "text",
        text: '[{"id":"cloudflare","description":"Workers","directory":"/skills/cloudflare"}]',
      },
    ],
  });
  expect(catalog.searchSkills).toHaveBeenCalledWith({
    skills: [
      {
        id: "cloudflare",
        description: "Workers",
        directory: "/skills/cloudflare",
      },
    ],
    query: "workers",
    limit: 5,
  });
});

it("loads the full skill only when requested", async () => {
  const client: Client = await connect();
  expect(
    await client.callTool({
      name: "read_skill",
      arguments: { id: "cloudflare" },
    }),
  ).toStrictEqual({
    content: [
      {
        type: "text",
        text: '{"id":"cloudflare","description":"Workers","directory":"/skills/cloudflare","text":"Instructions"}',
      },
    ],
  });
  expect(catalog.readText).toHaveBeenCalledWith("/skills/cloudflare/SKILL.md");
});

it("serves reference slices", async () => {
  const client: Client = await connect();
  expect(
    await client.callTool({
      name: "read_reference",
      arguments: { id: "cloudflare", path: "doc.md", offset: 0, limit: 10 },
    }),
  ).toStrictEqual({
    content: [
      {
        type: "text",
        text: '{"text":"Docs","totalCharacters":4,"nextOffset":null}',
      },
    ],
  });
});

it("reports errors without crashing the server", async () => {
  const client: Client = await connect();
  catalog.listSkills.mockRejectedValue(new Error("permission denied"));
  expect(
    await client.callTool({
      name: "read_skill",
      arguments: { id: "cloudflare" },
    }),
  ).toStrictEqual({
    isError: true,
    content: [{ type: "text", text: "permission denied" }],
  });
});

it("reports non-Error failures safely", async () => {
  const client: Client = await connect();
  catalog.listSkills.mockRejectedValue("unexpected");
  expect(
    await client.callTool({
      name: "read_skill",
      arguments: { id: "cloudflare" },
    }),
  ).toStrictEqual({
    isError: true,
    content: [{ type: "text", text: "Skill lookup failed" }],
  });
});

it("rejects invalid tool arguments before reading files", async () => {
  const client: Client = await connect();
  const response: Awaited<ReturnType<Client["callTool"]>> =
    await client.callTool({
      name: "search_skills",
      arguments: { query: "workers", limit: 100 },
    });
  expect(response.isError).toBe(true);
  expect(catalog.listSkills).not.toHaveBeenCalled();
});
