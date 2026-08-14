---
paths:
  - "**/*.ts"
  - "**/*.tsx"
  - "**/*.mts"
  - "**/*.cts"
---

# TypeScript Coding Rules

This is the only normative rule set for agents. Every requirement has one Rule ID and appears once.

## Repository and verification

- **R01 Repository setup:** Use the repository's configured runtime, package manager, workspace layout, and scripts. Do not add a second package manager or an unnecessary `package.json`.
- **R02 Verification toolchain:** After each logical change, run focused checks and then the configured type-check, tests, and exactly one complete lint/format toolchain: **Oxlint and Oxfmt together**, or **Biome**. Reproduce the expected behavior and resolve every failure before completion; running only Oxlint or only Oxfmt is incomplete.
- **R03 Quality integrity:** Do not suppress diagnostics, add ignore directives, or weaken checks without explicit approval.

## Type system

- **R04 Unsafe types:** Do not use `any` or unsafe narrowing with `as`; narrow with checks, predicates, generics, or corrected source types.
- **R05 Object shapes:** Use `interface` for object shapes with three or more properties and extract nested object shapes into appropriately scoped interfaces.
- **R06 Closed sets:** Prefer union types and do not introduce new `enum` declarations.
- **R07 Literal conformance:** Replace `as const` with `satisfies` or an explicit type.
- **R08 Type placement:** Place type and interface declarations before function implementations.

## Functions and control flow

- **R09 Function cohesion:** Keep functions focused, reuse production logic, and keep `main` or `run` as a thin orchestration layer.
- **R10 Return simplification:** Use an expression body for a clear single expression; when an `async` function only awaits one promise with no later work, return the promise directly.
- **R11 Nesting and ternaries:** Reduce nesting with guard clauses and use ternaries only for simple value selection; never nest or chain ternaries.
- **R12 Dispatch:** Replace repetitive `if`/`else if` or `switch` dispatch with a type-safe lookup object or `Map` when clearer.
- **R13 Production iteration:** In production code, prefer clear array methods over imperative loops and avoid nested loops.
- **R14 Parameters:** For three or more parameters, accept one typed object and destructure it in the parameter list when practical.
- **R15 Comparators:** Define reusable sort comparators separately from `sort` calls.

## Values, I/O, and modules

- **R16 Constants:** Give constants meaningful type information, replace fixed values and magic numbers with named constants, and place shared constants at module scope while keeping genuinely local constants local.
- **R17 Required values:** Validate missing required configuration or data and fail explicitly; use a fallback default only when the contract defines one.
- **R18 File I/O:** Minimize file reads and writes, batching or concurrently executing independent writes when ordering is unnecessary.
- **R19 Unused declarations:** Remove unused functions, variables, imports, and constants; use an underscore prefix only for a required but unused function parameter.
- **R20 Module boundaries:** Prefer named or default imports over namespace imports and do not create `index.ts` barrel files.
- **R21 Diagnostics language:** Write comments, logs, and error messages in English.
- **R22 Escaping:** Escape a literal `\n` correctly in files and string-based formats.

## External data and DOM

- **R23 HTML retrieval:** Prefer `fetch` unless an existing abstraction is more appropriate, and design parsing from inspected representative HTML rather than assumed markup.
- **R24 Character encoding:** Decode non-UTF-8 input explicitly with the repository's configured encoding library, such as `iconv-lite`.
- **R25 DOM environment:** Do not add `jsdom`; use the existing DOM test environment or prefer `happy-dom` when compatible emulation must be introduced.

## Tests

- **R26 Test ownership:** Add or update tests whenever TypeScript behavior or types change, following existing co-location and naming conventions such as `*.test.ts` or `*.test.tsx`.
- **R27 Coverage:** Meet the configured per-file coverage threshold without lowering it; if none exists, achieve at least 90% for every changed source file.
- **R28 Test isolation:** Keep unit tests deterministic and fast by mocking filesystem access and network requests.
- **R29 Test framework:** Use the existing test runner and imports; do not introduce another runner, including `bun:test`, unless the repository standardizes on it.
- **R30 Test case clarity:** Prefer explicit test cases over abstractions or loops that hide inputs, assertion paths, or expected results.
- **R31 Suite structure:** Minimize `describe` nesting and unnecessary suite wrappers.
- **R32 Assertions:** Prefer `toStrictEqual` over `toEqual`; do not use `toContain` or `expect(value.includes(item)).toBe(true)`; write expected strings, arrays, and objects as explicit literals independent of the implementation.
