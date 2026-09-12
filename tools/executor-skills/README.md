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

## Agent operations (separate integrations)

The `agmsg` integration runs `agmsg-cli.mjs` and exposes only identity lookup, team,
history, one-time inbox and sending through the official installed Bash scripts.
It is separate from the unchanged read-only `local-skills` server. Requests carry
an explicit originating project and identity; send/inbox/history validate that
identity against `identities.sh` for type `pi`. No team/config/SQLite files are read
by this adapter. Arguments use `execFile`, never shell interpolation. Script calls
have a ten-second deadline and a bounded buffer; responses are capped at 24,000
characters and explicitly indicate truncation. Inbox consumes unread messages and
send writes messages, so both are correctly annotated as mutations, not read-only.

The `ctx` integration uses the installed official `ctx mcp serve`, not another
CLI wrapper. Its tool schemas are discovered on demand. Keep explicit workspace
and session filters because the shared server is not the originating Pi session.

The package's `pi.ts` disables only Pi's direct model-callable `agmsg` tool on
session start/reload. The original extension's automatic delivery and `/agmsg`
commands remain intact. Before each agent run, the router reads only Pi's existing
`agmsg-active-identity` custom session entries and adds the selected identity and
originating project to the prompt, preserving the same name used by automatic
inbox delivery. Cleared identities are respected; no agmsg storage is read. Neither native tool schemas nor the two individual skill
descriptions need to remain in Pi's startup prompt.

Run `../../scripts/setup-executor.sh --approve-registration` to provision them.
This permits only registration/connection creation, not a permanent tool approval.
For a new Mac, follow [SETUP.ja.md](SETUP.ja.md): `--install-service` installs the
bundled runtime with this checkout's explicit scope, and paths are resolved locally.
No database, credentials, or live approval policies are exported or synced.
The CLI wrapper keeps its scope independent of the invoking agent's cwd.
Use read-only calls for verification; do not send test messages to real agents.
`bun run test:setup` runs offline shell tests for portable paths, service setup,
repeatability, and approval/error handling; it is included in `bun run test`.
