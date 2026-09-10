// Runs with Bun.
import { readdir, readFile, realpath, stat } from "node:fs/promises";
import { isAbsolute, join, relative, sep } from "node:path";
import { parse } from "yaml";
import { z } from "zod";

export interface Skill {
  id: string;
  description: string;
  directory: string;
}

export interface SearchRequest {
  skills: Skill[];
  query: string;
  limit: number;
}

export interface ReferenceRequest {
  id: string;
  path: string;
  offset: number;
  limit: number;
}

export interface ReferenceResult {
  text: string;
  totalCharacters: number;
  nextOffset: number | null;
}

const MAX_FILE_BYTES: number = 262_144;
const MAX_PAGE_CHARACTERS: number = 24_000;
const MAX_SEARCH_RESULTS: number = 20;
const FRONTMATTER: RegExp = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/;
const METADATA: z.ZodObject<{ description: z.ZodString }> = z.object({
  description: z.string().trim().min(1),
});

const compareText = (left: string, right: string): number =>
  left.localeCompare(right);

const isMissing = (error: unknown): boolean =>
  error instanceof Error && "code" in error && error.code === "ENOENT";

export const readText = async (path: string): Promise<string> => {
  if ((await stat(path)).size > MAX_FILE_BYTES) {
    throw new Error("Skill file exceeds 256 KiB");
  }
  return readFile(path, "utf8");
};

const readEntry = async (path: string): Promise<Skill[]> => {
  try {
    if (!(await stat(path)).isDirectory()) return [];
    const directory: string = await realpath(path);
    const text: string = await readText(join(directory, "SKILL.md"));
    const header: string = text.match(FRONTMATTER)?.[1] ?? "";
    const parsed: unknown = parse(header);
    const metadata: ReturnType<typeof METADATA.safeParse> =
      METADATA.safeParse(parsed);
    if (!metadata.success) return [];
    return [
      {
        id: path.split(sep).at(-1) ?? path,
        description: metadata.data.description,
        directory,
      },
    ];
  } catch (error: unknown) {
    if (isMissing(error)) return [];
    throw error;
  }
};

const readRoot = async (root: string): Promise<Skill[]> => {
  try {
    const entries: string[] = (await readdir(root)).sort(compareText);
    const groups: Skill[][] = await Promise.all(
      entries
        .filter((name: string) => !name.startsWith("."))
        .map(async (name: string) => {
          const path: string = join(root, name);
          return readEntry(path);
        }),
    );
    return groups.flat();
  } catch (error: unknown) {
    if (isMissing(error)) return [];
    throw error;
  }
};

export const listSkills = async (roots: string[]): Promise<Skill[]> => {
  const groups: Skill[][] = await Promise.all(roots.map(readRoot));
  return groups
    .flat()
    .filter(
      (skill: Skill, index: number, all: Skill[]) =>
        all.findIndex((candidate: Skill) => candidate.id === skill.id) ===
        index,
    );
};

export const searchSkills = ({
  skills,
  query,
  limit,
}: SearchRequest): Skill[] => {
  if (!Number.isInteger(limit) || limit < 1 || limit > MAX_SEARCH_RESULTS) {
    throw new Error("Search limit must be an integer between 1 and 20");
  }
  const words: string[] = query.toLowerCase().split(/\s+/).filter(Boolean);
  return skills
    .filter((skill: Skill) =>
      words.every((word: string) =>
        `${skill.id} ${skill.description}`.toLowerCase().includes(word),
      ),
    )
    .slice(0, limit);
};

export const selectSkill = (skills: Skill[], id: string): Skill => {
  const skill: Skill | undefined = skills.find(
    (candidate: Skill) => candidate.id === id,
  );
  if (!skill) throw new Error("Unknown skill ID; search the catalog first");
  return skill;
};

export const readReference = async (
  skills: Skill[],
  request: ReferenceRequest,
): Promise<ReferenceResult> => {
  if (
    !Number.isInteger(request.offset) ||
    request.offset < 0 ||
    !Number.isInteger(request.limit) ||
    request.limit < 1 ||
    request.limit > MAX_PAGE_CHARACTERS
  ) {
    throw new Error("Invalid reference offset or limit");
  }
  if (isAbsolute(request.path))
    throw new Error("Reference path must be relative");
  const skill: Skill = selectSkill(skills, request.id);
  const file: string = await realpath(join(skill.directory, request.path));
  const inside: string = relative(skill.directory, file);
  if (inside === ".." || inside.startsWith(`..${sep}`) || isAbsolute(inside)) {
    throw new Error("Reference escapes the selected skill directory");
  }
  const text: string = await readText(file);
  const end: number = request.offset + request.limit;
  return {
    text: text.slice(request.offset, end),
    totalCharacters: text.length,
    nextOffset: end < text.length ? end : null,
  };
};
