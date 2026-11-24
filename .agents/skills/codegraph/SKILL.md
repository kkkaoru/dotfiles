---
name: codegraph
description: Use the CodeGraph index for structural code questions — how a piece of code works, how X reaches Y, who calls a symbol, or the blast radius of a change — in repositories that have a `.codegraph/` directory. Prefer it over grep/find and reading files.
---

# CodeGraph

In a repository indexed by CodeGraph (a `.codegraph/` directory exists at the repo root),
reach for it BEFORE `grep`/`find` or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call —
  the relevant symbols' verbatim source plus the call paths between them, including
  dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its
  current line-numbered source. If it is listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

Treat the returned source as already read and do not re-verify it with grep; auto-sync keeps the
index fresh while files change, and a staleness banner appears after edits.

`codegraph init` builds a project's index (one time, per project). If there is no `.codegraph/`
directory, skip CodeGraph entirely — indexing is the user's decision.
