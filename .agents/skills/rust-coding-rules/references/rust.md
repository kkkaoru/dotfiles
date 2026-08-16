---
paths:
  - "**/*.rs"
---

# Rust Coding Rules

This is the only normative Rust rule set for agents. Every requirement has one Rule ID and appears once.

## Repository and verification

- **RS01 Repository setup:** Use the repository's configured Rust toolchain, Cargo workspace, feature layout, and scripts. Do not add a second build system or unnecessary crate.
- **RS02 Verification toolchain:** After each logical change, run focused checks and then the configured formatting, compilation, Clippy, and test commands. Use `rustfmt` and Clippy together when they are the configured quality tools, reproduce the expected behavior, and resolve every failure before completion.
- **RS03 Quality integrity:** Do not add `allow` attributes, ignore directives, skipped tests, or weaker quality gates without explicit approval.

## Safety, types, and APIs

- **RS04 Unsafe code:** Avoid `unsafe`. When it is unavoidable, minimize its scope, document every safety invariant, expose a safe API, and test boundary conditions.
- **RS05 Error propagation:** In production paths, return structured `Result` errors and use `?` or explicit handling instead of `unwrap`, `expect`, or panic-driven control flow.
- **RS06 Domain types:** Use structs, enums, and newtypes to encode valid states and units rather than passing loosely related primitives.
- **RS07 Abstraction:** Prefer concrete types and small traits; introduce generics, trait objects, or macros only when they remove demonstrated duplication or provide a required boundary.
- **RS08 Visibility:** Keep items private by default and expose the smallest stable public API needed by callers.

## Functions and control flow

- **RS09 Function cohesion:** Keep functions focused, reuse production logic, keep non-test code as DRY as possible, and keep `main` or runtime entry points as thin orchestration layers with as few steps as possible.
- **RS10 Branching:** Reduce nesting with early returns, `match`, `if let`, and `let else`, selecting the construct that makes all states explicit.
- **RS11 Iteration:** Prefer iterator adapters when they improve clarity, but use a loop when mutation, short-circuiting, or stateful control flow is clearer; avoid double or deeper loop nesting inside a single function.
- **RS12 Parameters:** Group cohesive parameters into a typed struct when a function has 3 or more related parameters or when that clarifies invariants or call sites; do not create parameter structs solely to satisfy a numeric threshold.
- **RS13 Async boundaries:** Do not mark functions `async` without asynchronous work, and do not perform blocking I/O or hold blocking locks across `.await`.
- **RS28 Implementation planning:** Build a thorough implementation plan before implementing.
- **RS29 Comparators:** Define reusable sort keys or comparison functions separately from the `sort` or `sort_by` call.

## Ownership, mutation, and concurrency

- **RS14 Ownership:** Borrow when ownership transfer is unnecessary and avoid cloning solely to silence borrow-checker errors; make intentional ownership boundaries explicit.
- **RS15 Mutation:** Keep mutability local and minimal, and avoid mutable global state.
- **RS16 Concurrency:** Use the narrowest synchronization primitive that preserves correctness, minimize lock scope, and never hold a lock longer than required.

## Values, modules, and diagnostics

- **RS17 Constants:** Replace fixed values and magic numbers with typed named constants, placing shared constants at module scope and genuinely local constants locally. Do not define shared constants inside a function.
- **RS18 Required values:** Validate missing configuration or input and return an explicit error; use a fallback only when the contract defines one.
- **RS19 Modules and imports:** Keep modules cohesive, prefer explicit imports, and avoid broad glob re-exports except in a deliberate prelude or tightly scoped test module.
- **RS20 Dead code:** Remove unused items, imports, feature branches, and obsolete compatibility paths rather than silencing warnings. Either delete or use unused names; do not silence unused locals with an underscore prefix. Underscores are allowed only for required unused parameters.
- **RS21 Documentation and diagnostics:** Write comments, public API documentation, logs, and error messages in English, documenting rationale and invariants rather than restating syntax.
- **RS30 Declaration order:** Place type, struct, enum, trait, and module-level constant declarations before function and `impl` method implementations. Do not insert these declarations between functions.
- **RS31 Rules review:** After implementation, automatically review whether the change follows this rule set.

## Resources and data

- **RS22 Resource handling:** Use RAII and scoped guards for files, locks, temporary state, and cleanup; minimize reads and writes, buffer or batch independent I/O when ordering is unnecessary, and escape literal `\n` correctly when writing it as text.
- **RS23 Data boundaries:** Validate external data before constructing domain types and handle text encoding, parsing, and serialization errors explicitly.

## Tests and coverage

- **RS24 Test ownership:** Add or update unit or integration tests whenever Rust behavior or public types change, following the repository's existing test layout. When creating, editing, changing, or deleting a non-test Rust file, always update the related tests.
- **RS25 Coverage:** Meet the configured per-file coverage threshold without lowering it; if none exists, achieve at least 90% for every changed source file when coverage tooling is available.
- **RS26 Test isolation:** Keep tests deterministic and fast by isolating filesystem, clock, process, and network dependencies with temporary resources or test doubles. Implement tests with execution speed as a priority as well.
- **RS27 Test clarity:** Do not write DRY test code. Use the existing test framework, prefer explicit cases over shared helpers and loops, ban `for` loops in tests, and ban assertions inside a loop. Minimize nested test modules. Do not pass variables, constants, or interpolated strings as expected values; write explicit inputs and expected literals, choose assertion macros that show useful diffs, and avoid helper abstractions that hide the behavior under test.
