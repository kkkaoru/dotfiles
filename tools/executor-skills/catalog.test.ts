// Runs with Bun; Vitest mocks all filesystem I/O.
import { beforeEach, expect, it, type Mock, vi } from "vitest";
import {
  listSkills,
  readReference,
  readText,
  type Skill,
  searchSkills,
  selectSkill,
} from "./catalog.ts";

interface FileInfo {
  size: number;
  isDirectory: () => boolean;
}
interface MockFiles {
  readdir: Mock<(path: string) => Promise<string[]>>;
  stat: Mock<(path: string) => Promise<FileInfo>>;
  readFile: Mock<(path: string, encoding: string) => Promise<string>>;
  realpath: Mock<(path: string) => Promise<string>>;
}

const io: MockFiles = vi.hoisted(() => ({
  readdir: vi.fn(),
  stat: vi.fn(),
  readFile: vi.fn(),
  realpath: vi.fn(),
}));
const skills: Skill[] = [
  {
    id: "cloudflare",
    description: "Workers and storage",
    directory: "/skills/cloudflare",
  },
];
vi.mock("node:fs/promises", () => io);

beforeEach(() => {
  vi.resetAllMocks();
  io.stat.mockResolvedValue({ size: 10, isDirectory: () => true });
  io.realpath.mockImplementation(async (path: string) => path);
  io.readdir.mockResolvedValue(["cloudflare"]);
  io.readFile.mockResolvedValue(
    "---\nname: cloudflare\ndescription: Workers and storage\n---\nInstructions",
  );
});

it("discovers metadata, deduplicates roots and ignores dot entries", async () => {
  io.readdir.mockResolvedValue(["cloudflare", ".hidden"]);
  expect(await listSkills(["/skills", "/skills"])).toStrictEqual([
    {
      id: "cloudflare",
      description: "Workers and storage",
      directory: "/skills/cloudflare",
    },
  ]);
});

it("sorts entries and parses multiline descriptions", async () => {
  io.readdir.mockResolvedValue(["z", "a"]);
  io.readFile.mockResolvedValue(
    "---\ndescription: >-\n  two\n  lines\n---\nBody",
  );
  expect(
    (await listSkills(["/skills"])).map((skill: Skill) => skill.id),
  ).toStrictEqual(["a", "z"]);
  expect((await listSkills(["/skills"]))[0]?.description).toBe("two lines");
});

it("ignores a missing root", async () => {
  io.readdir.mockRejectedValue(
    Object.assign(new Error("missing"), { code: "ENOENT" }),
  );
  expect(await listSkills(["/missing"])).toStrictEqual([]);
});

it("ignores a missing SKILL.md without dropping other entries", async () => {
  io.readdir.mockResolvedValue(["empty", "cloudflare"]);
  io.readFile.mockImplementation(async (path: string) => {
    if (path.endsWith("empty/SKILL.md"))
      throw Object.assign(new Error("missing"), { code: "ENOENT" });
    return "---\ndescription: Workers\n---\nBody";
  });
  expect(
    (await listSkills(["/skills"])).map((skill: Skill) => skill.id),
  ).toStrictEqual(["cloudflare"]);
});

it("ignores plain files", async () => {
  io.stat.mockResolvedValue({ size: 10, isDirectory: () => false });
  expect(await listSkills(["/skills"])).toStrictEqual([]);
});

it.each([
  "No frontmatter",
  "---\nname: only\n---\nBody",
  "---\ndescription: ''\n---\nBody",
])("ignores missing descriptions: %s", async (text: string) => {
  io.readFile.mockResolvedValue(text);
  expect(await listSkills(["/skills"])).toStrictEqual([]);
});

it("does not swallow permission errors", async () => {
  io.readFile.mockRejectedValue(
    Object.assign(new Error("permission denied"), { code: "EACCES" }),
  );
  await expect(listSkills(["/skills"])).rejects.toThrow("permission denied");
});

it("surfaces malformed YAML", async () => {
  io.readFile.mockResolvedValue("---\ndescription: [broken\n---\nBody");
  await expect(listSkills(["/skills"])).rejects.toThrow();
});

it("rejects large files before reading", async () => {
  io.stat.mockResolvedValue({ size: 262145, isDirectory: () => false });
  await expect(readText("/huge")).rejects.toThrow("Skill file exceeds 256 KiB");
  expect(io.readFile).not.toHaveBeenCalled();
});

it("searches case insensitively with AND terms and bounds results", () => {
  expect(
    searchSkills({ skills, query: "CLOUDFLARE workers", limit: 1 }).map(
      (skill: Skill) => skill.id,
    ),
  ).toStrictEqual(["cloudflare"]);
  expect(searchSkills({ skills, query: "missing", limit: 1 })).toStrictEqual(
    [],
  );
  expect(searchSkills({ skills, query: "", limit: 1 }).length).toBe(1);
});

it.each([0, 21, 1.5, Number.NaN])(
  "rejects invalid search limit %s",
  (limit: number) => {
    expect(() => searchSkills({ skills, query: "", limit })).toThrow(
      "Search limit",
    );
  },
);

it("selects an exact skill and rejects unknown IDs", () => {
  expect(selectSkill(skills, "cloudflare").directory).toBe(
    "/skills/cloudflare",
  );
  expect(() => selectSkill(skills, "../secret")).toThrow("Unknown skill ID");
});

it("paginates references and signals completion", async () => {
  io.readFile.mockResolvedValue("abcdef");
  expect(
    await readReference(skills, {
      id: "cloudflare",
      path: "references/doc.md",
      offset: 0,
      limit: 3,
    }),
  ).toStrictEqual({ text: "abc", totalCharacters: 6, nextOffset: 3 });
  expect(
    await readReference(skills, {
      id: "cloudflare",
      path: "references/doc.md",
      offset: 3,
      limit: 3,
    }),
  ).toStrictEqual({ text: "def", totalCharacters: 6, nextOffset: null });
});

it.each([
  { offset: -1, limit: 1 },
  { offset: 0.5, limit: 1 },
  { offset: 0, limit: 0 },
  { offset: 0, limit: 24001 },
  { offset: 0, limit: 1.5 },
])("rejects invalid slices %s", async (slice) => {
  await expect(
    readReference(skills, { id: "cloudflare", path: "doc.md", ...slice }),
  ).rejects.toThrow("Invalid reference");
});

it("rejects absolute paths before I/O", async () => {
  await expect(
    readReference(skills, {
      id: "cloudflare",
      path: "/etc/passwd",
      offset: 0,
      limit: 10,
    }),
  ).rejects.toThrow("must be relative");
  expect(io.realpath).not.toHaveBeenCalled();
});

it.each(["/skills/secret", "/skills", "/skills/cloudflare-other/secret"])(
  "rejects traversal and escaping symlinks to %s",
  async (destination: string) => {
    io.realpath.mockResolvedValue(destination);
    await expect(
      readReference(skills, {
        id: "cloudflare",
        path: "ref.md",
        offset: 0,
        limit: 10,
      }),
    ).rejects.toThrow("escapes");
    expect(io.readFile).not.toHaveBeenCalled();
  },
);

it("permits references in the canonical target of a trusted skill symlink", async () => {
  io.realpath
    .mockResolvedValueOnce("/installed/cloudflare")
    .mockResolvedValueOnce("/installed/cloudflare/doc.md");
  io.readFile
    .mockResolvedValueOnce("---\ndescription: Workers\n---\nBody")
    .mockResolvedValueOnce("Docs");
  expect(
    await readReference(await listSkills(["/skills"]), {
      id: "cloudflare",
      path: "doc.md",
      offset: 0,
      limit: 10,
    }),
  ).toStrictEqual({ text: "Docs", totalCharacters: 4, nextOffset: null });
});
