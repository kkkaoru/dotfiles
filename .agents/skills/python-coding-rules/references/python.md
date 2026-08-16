---
paths:
  - "**/*.py"
  - "**/*.pyi"
---

# Python Coding Rules

This is the only normative Python rule set for agents. Every requirement has one Rule ID and appears once.

## Repository and verification

- **PY01 Repository setup:** Use the repository's configured Python version, environment manager, dependency files, package layout, and scripts. Do not introduce a second environment or package manager.
- **PY02 Verification toolchain:** After each logical change, run focused checks and then the configured type-check, lint, format-check, and tests. Use the complete configured lint/format workflow, such as Ruff check and Ruff format together, reproduce the expected behavior, and resolve every failure before completion.
- **PY03 Quality integrity:** Do not add `noqa`, `type: ignore`, skipped tests, warning filters, or weaker quality gates without explicit approval.

## Types and data models

- **PY04 Unsafe typing:** Avoid `Any` and unchecked `cast`; narrow with runtime checks, `TypeGuard`, protocols, generics, or corrected source annotations.
- **PY05 Function contracts:** Type public functions, methods, return values, and externally visible attributes, preserving useful generic information.
- **PY06 Structured data:** Use the representation that matches the boundary: dataclasses or validated models for owned records, `TypedDict` for dictionary-shaped data, and `Protocol` for behavioral interfaces.
- **PY07 Optional values:** Model absence explicitly with `None` unions and narrow before use rather than relying on truthiness when valid false-like values exist.
- **PY08 Defaults:** Never use a mutable object as a function or dataclass default; use `None`, `default_factory`, or an immutable value according to the contract.

## Functions and control flow

- **PY09 Function cohesion:** Keep functions focused, reuse production logic, keep non-test code as DRY as possible, and keep CLI or application entry points as thin orchestration layers with as few steps as possible.
- **PY10 Branching:** Reduce nesting with early returns and guard clauses; use conditional expressions only for clear, simple values and never nest them.
- **PY11 Expressions:** Use comprehensions only when they remain readable; replace complex or nested comprehensions with named steps.
- **PY12 Iteration:** Use iterators and generators for streaming or large data, avoid materializing collections without need, and avoid double or deeper loop nesting inside a single function.
- **PY13 Parameters:** Use keyword-only parameters or a typed configuration object when a call has several related options or 3 or more parameters; do not hide required inputs in ambient global state.
- **PY14 Async boundaries:** Do not mark functions `async` without asynchronous work, and do not call blocking I/O directly from an event loop.
- **PY30 Implementation planning:** Build a thorough implementation plan before implementing.
- **PY31 Dispatch:** Replace repeated `if`/`elif` dispatch with a typed lookup dict or mapping when that is clearer.
- **PY32 Comparators:** Define reusable sort keys or comparison functions separately from the `sorted` or `list.sort` call.

## Values, errors, and resources

- **PY15 Constants and globals:** Give constants meaningful type information, name them in uppercase at module scope, replace magic numbers with named constants, and avoid mutable global state. Do not define shared constants inside a function.
- **PY16 Required values:** Validate missing configuration or input and raise or return an explicit domain error; use a fallback only when the contract defines one.
- **PY17 Exceptions:** Catch the narrowest exception type, preserve causes with `raise ... from ...`, and never use bare `except` or silently swallow failures.
- **PY18 Runtime validation:** Do not use `assert` for production input validation or recoverable runtime errors.
- **PY19 Resource handling:** Use context managers for files, locks, temporary resources, transactions, and cleanup.
- **PY20 File I/O:** Use `pathlib`, specify text encoding explicitly, minimize reads and writes, write independent files in parallel when ordering is unnecessary, and escape literal `\n` correctly when writing it as text.

## Modules and diagnostics

- **PY21 Imports:** Use explicit imports, avoid `from module import *`, keep imports at module scope unless deferral is required, prevent circular dependencies through better module boundaries, and do not create barrel modules that only re-export other modules.
- **PY22 Module scope:** Keep modules cohesive and split them when unrelated responsibilities or excessive size make testing and navigation difficult.
- **PY23 Unused code:** Remove unused functions, variables, imports, compatibility branches, and obsolete code instead of silencing tooling. Either delete or use unused names; do not silence unused locals with an underscore prefix. Underscores are allowed only for required unused parameters.
- **PY24 Documentation and diagnostics:** Write comments, docstrings, logs, and error messages in English, documenting contracts and rationale rather than restating syntax.
- **PY33 Declaration order:** Place type aliases, `TypedDict`, `Protocol`, dataclasses, Enums, and module-level constants before function implementations. Do not insert these declarations between functions.
- **PY34 Rules review:** After implementation, automatically review whether the change follows this rule set.

## Tests and coverage

- **PY25 Test ownership:** Add or update tests whenever Python behavior, public types, or data models change, following the repository's existing test layout and naming. When creating, editing, changing, or deleting a non-test Python file, always update the related tests.
- **PY26 Coverage:** Meet the configured per-file coverage threshold without lowering it; if none exists, achieve at least 90% for every changed source file when coverage tooling is available.
- **PY27 Test isolation:** Keep tests deterministic and fast by isolating filesystem, clock, environment, process, and network dependencies with fixtures, temporary paths, monkeypatching, or test doubles. Implement tests with execution speed as a priority as well.
- **PY28 Test framework:** Use the repository's existing test framework and fixtures rather than introducing another runner or assertion library.
- **PY29 Test clarity:** Do not write DRY test code. Prefer explicit cases over shared helpers and loops, ban `for` loops in tests, and ban assertions inside a loop. Minimize nested test classes and suite wrappers. Do not pass variables, constants, or interpolated strings as expected values; assert complete outcomes with explicit literals instead of implementation details.
