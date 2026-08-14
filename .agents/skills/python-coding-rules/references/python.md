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

- **PY09 Function cohesion:** Keep functions focused, reuse production logic, and keep CLI or application entry points as thin orchestration layers.
- **PY10 Branching:** Reduce nesting with early returns and guard clauses; use conditional expressions only for clear, simple values and never nest them.
- **PY11 Expressions:** Use comprehensions only when they remain readable; replace complex or nested comprehensions with named steps.
- **PY12 Iteration:** Use iterators and generators for streaming or large data, and avoid materializing collections without need.
- **PY13 Parameters:** Use keyword-only parameters or a typed configuration object when a call has several related options; do not hide required inputs in ambient global state.
- **PY14 Async boundaries:** Do not mark functions `async` without asynchronous work, and do not call blocking I/O directly from an event loop.

## Values, errors, and resources

- **PY15 Constants and globals:** Name constants in uppercase at module scope and avoid mutable global state.
- **PY16 Required values:** Validate missing configuration or input and raise or return an explicit domain error; use a fallback only when the contract defines one.
- **PY17 Exceptions:** Catch the narrowest exception type, preserve causes with `raise ... from ...`, and never use bare `except` or silently swallow failures.
- **PY18 Runtime validation:** Do not use `assert` for production input validation or recoverable runtime errors.
- **PY19 Resource handling:** Use context managers for files, locks, temporary resources, transactions, and cleanup.
- **PY20 File I/O:** Use `pathlib`, specify text encoding explicitly, and batch or stream I/O when that avoids unnecessary memory or operations.

## Modules and diagnostics

- **PY21 Imports:** Use explicit imports, avoid `from module import *`, keep imports at module scope unless deferral is required, and prevent circular dependencies through better module boundaries.
- **PY22 Module scope:** Keep modules cohesive and split them when unrelated responsibilities or excessive size make testing and navigation difficult.
- **PY23 Unused code:** Remove unused functions, variables, imports, compatibility branches, and obsolete code instead of silencing tooling.
- **PY24 Documentation and diagnostics:** Write comments, docstrings, logs, and error messages in English, documenting contracts and rationale rather than restating syntax.

## Tests and coverage

- **PY25 Test ownership:** Add or update tests whenever Python behavior, public types, or data models change, following the repository's existing test layout and naming.
- **PY26 Coverage:** Meet the configured per-file coverage threshold without lowering it; if none exists, achieve at least 90% for every changed source file when coverage tooling is available.
- **PY27 Test isolation:** Keep tests deterministic and fast by isolating filesystem, clock, environment, process, and network dependencies with fixtures, temporary paths, monkeypatching, or test doubles.
- **PY28 Test framework:** Use the repository's existing test framework and fixtures rather than introducing another runner or assertion library.
- **PY29 Test clarity:** Prefer explicit parameterization and expected literals, assert complete outcomes instead of implementation details, and avoid helper abstractions that obscure the behavior under test.
