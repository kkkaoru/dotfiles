# Executor local skills (Bun)

Read-only MCP integration for installed Agent Skills. Executor indexes three
tools, not every skill description. Pi keeps one routing skill in its prompt.

Pi connects through its native MCP client using `.pi/agent/mcp.json` →
`~/.local/bin/executor mcp --elicitation-mode browser`. This uses the existing
Executor Desktop catalog, credentials and approval policies; no second runtime,
registration migration or OAuth token copying is needed. Tools use Pi's default
codemode exposure: discover the bridge schema, then discover the integration schema.
The CLI instructions below remain available for other clients and administration.
Run `pi mcp list` to verify connectivity and `/reload` (or start a new session) to
activate the config and updated router. Other clients' `.mcp.json` is unchanged.

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

The setup recipe includes `~/.agents/skills-stroage` as a read-only catalog root
alongside generic skill roots. This does not activate every stored skill in agent
startup discovery. Use the generic `project-skills` skill to copy a reviewed
selection into the target project's `.agents/skills`. Existing registrations are
not automatically changed by filesystem reorganization; setup requires its usual
explicit registration approval. Project-local customized copies should be read
from that project, not silently replaced by a catalog version.

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

The `ctx` integration uses the installed official `ctx mcp serve`, not another
CLI wrapper. Its tool schemas are discovered on demand. Keep explicit workspace
and session filters because the shared server is not the originating Pi session.

The package ships only the read-only skill router: it exposes `./skills` to Pi and
registers no extension of its own.

Run `../../scripts/setup-executor.sh --approve-registration` to provision them.
This permits only registration/connection creation, not a permanent tool approval.
For a new Mac, follow [SETUP.ja.md](SETUP.ja.md): `--install-service` installs the
bundled runtime with this checkout's explicit scope, and paths are resolved locally.
No database, credentials, or live approval policies are exported or synced.
The CLI wrapper keeps its scope independent of the invoking agent's cwd.
Use read-only calls for verification; do not send test messages to real agents.
`bun run test:setup` runs offline shell tests for portable paths, service setup,
repeatability, and approval/error handling; it is included in `bun run test`.
