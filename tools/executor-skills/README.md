# Executor local skills (Bun)

Read-only MCP integration for installed Agent Skills. Executor indexes three
tools, not every skill description. Pi keeps one routing skill in its prompt.

Implementation plan:
1. Discover explicitly configured, local skill roots; deduplicate by ID.
2. Search bounded name/description metadata; load instructions only on request.
3. Read references only inside the selected skill's canonical directory.
4. Expose no file writes, shell execution, network fetches, or credential tools.
5. Verify with mocked filesystem tests, typecheck, Biome and >=90% coverage.

```sh
bun install
bun run tsc && bun run lint && bun run test
bun run cli.mjs /absolute/path/to/skills /another/skills
```

Each root contains immediate child directories with `SKILL.md`. Symlinked
skill directories are supported: supplying a root trusts its installed child
skills. Reference symlinks escaping a skill directory are rejected. Missing
roots are skipped; permission and other I/O errors are not hidden. IDs use the
directory name (stable even for invalid/mismatched frontmatter names).

`search_skills` requires a query and limit (1–20); an empty query lists a bounded
page. `read_skill` returns the full SKILL.md (maximum 256 KiB) and canonical base
path. `read_reference` takes a relative path and a bounded slice (maximum 24,000
characters), so longer references can be paged through without truncation loss.

These tools return instructions, not executable actions. Load the selected
skill, then separately discover the Cloudflare/MotherDuck/etc. integration in
Executor. Instructions cannot bypass tool approvals or authorize data writes.
