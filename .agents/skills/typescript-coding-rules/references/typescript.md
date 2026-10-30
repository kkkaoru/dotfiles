---
paths:
  - "**/*.ts"
  - "**/*.tsx"
  - "**/*.mts"
  - "**/*.cts"
---

TypeScript coding rules. Follow every item at all times.

## Repository and verification

1. State in a file comment that the file runs with bun.
2. Consolidate into package.json when possible.
3. Build a thorough implementation plan before implementing.
4. Resolve `bun run tsc` errors and keep `bun run tsc` error-free at all times.
5. Run `bun run tsc` after every unit of work and fix any errors.
6. Resolve lint warnings and errors and keep lint warning-free and error-free at all times.
7. After changing code, confirm that `bun run tsc`, lint, and `bun run test` always succeed.
8. After changing test code, confirm that `bun run tsc`, lint, and `bun run test` always succeed.
9. Always run `bun run test` after editing a `*.ts` file.
10. Keep fixing until verification produces the expected result.
11. After changing code, always run verification when the verification command is known.
12. Always automatically review whether the implementation follows typescript-rules.ja.mdx.

## Types

13. Ban `any`.
14. Ban forcing types with `as`.
15. Ban `as const`. Use `satisfies`.
16. Always define types with `interface` when `interface` can be used.
17. Define an `interface` for objects with 3 or more properties.
18. If an `interface` type has nested parts, add more `interface` definitions in an appropriate scope.
19. Keep `interface` definitions at the top of the file. Ban defining an `interface` between function definitions.
20. Prefer union types.
21. Ban introducing new `enum`s.
22. Ban `const` definitions that have no type information.

## Functions and control flow

23. Prefer splitting functions, normalizing them, and writing DRY code. Keep calculation apart from side effects: a function that decides a value (`dynamicEffort`, `nextFloor`, `countCompaction`, `withObservation`) takes plain data and returns the result, without host objects (`engine`, `pi`, `context.ui`) or state writes, and a separate small function applies it (assigns state, calls the host). The decision is then tested with plain values and no fake host. Split the same way when one function both records an event and decides from it (`recordMainStep` then `managedEffort`). When a side effect is needed only on a change, write the caller as a ternary that returns the unchanged value directly and passes a change to the applying function: `const sendEffort = (engine, effort): ModelEffort => effort === lastSent ? effort : announceEffort(engine, effort)`.
24. Keep non-test code as DRY as possible.
25. When a `main` or `run` function exists, split the work so that these functions have as few steps as possible.
26. When creating or defining a `main` function, reduce definitions inside the `main` function scope as much as possible, split function definitions, and make coverage easier to raise.
27. When a function has 3 or more parameters, take one object as the argument.
28. In arrow functions used as callbacks or returned as values, use the concise body to omit `return` when it can be omitted.
29. Minimize `return` and `const` definitions within a readable range. Do not bind a value to a `const` that is read only once: use the expression where it is needed. When inlining leaves a function body that is a single `return`, write it as a concise-body `const` arrow function (rule 40), for example `const noteToolResult = (…): R => recordCall(…) ? { …result, context } : result` instead of `const stuck = recordCall(…); return stuck ? … : result`. When the single-use value is the receiver of the next statement, continue the method chain instead (`messages.filter(…).slice(-n).forEach(…)`, not `const rounds = messages.filter(…); rounds.slice(-n).forEach(…)`); likewise pass a single-use object literal or call result directly as the argument. Keep a single-use `const` only when it must be evaluated before a side effect that would change it (for example reading `ring.items.includes(key)` before writing the ring, or an `await` that must finish before a check; there, pass the awaited value as an argument to a function that performs the check, `replayIfCurrent(generation, await $.session.messages())`, since arguments are evaluated before the call, and do not move the `await` inside the guarded branch, which would run the check first), when it is read inside a callback or loop body where inlining would evaluate it once per item, or when its name explains a value the expression alone does not.
    When a body needs several statements only because it reads state and then updates it, change the state so the body becomes one expression; never use the comma operator (`no-sequences: "error"`). Derive a one-time condition from state that already exists instead of clearing a flag (`limits.size === 0 && prefetched !== undefined`, not reading `prefetched` and then assigning `prefetched = undefined`). Move the repeated "store and return" pair (`map.set(key, value); return value`) into one helper that the callers share (`rememberLimit(model, limit)`), so each caller becomes a concise-body `const` arrow function (`const fetchLimit = async (…): Promise<number> => rememberLimit(model, await sessionLimit($))`). When the store call returns its always-truthy receiver (`Map#set`, `Set#add`), write that helper itself as one expression: `const rememberLimit = (model: string, limit: number): number => limits.set(model, limit) && limit`, with a one-line comment that the receiver is always truthy. Confirm the one-time behaviour with a test, for example that a second key does not reuse the value.
30. Minimize nesting in all code.
31. Use a guard to simplify nested logic when possible. Ban replacing a ternary operator with a guard. Never omit the braces of an `if`, `else` or loop body, including a one-line guard such as `if (done) { return value }`. Enforce this mechanically: oxlint/ESLint `curly: ["error", "all"]` or Biome `style/useBlockStatements: "error"`, applied with `--fix`; formatters (Oxfmt, Prettier, Biome format) cannot add braces.
32. Use a ternary operator whenever possible. When a function only guards and then assigns state, make it return the new value (the current value when nothing changes) and assign it at the caller, so the function becomes a ternary: `runtime.manager.contextTokens = contextAfter(agentId, usage)` with `const contextAfter = (…): number => agentId !== undefined || usage === null ? runtime.manager.contextTokens : …`.
    Do not assign a value back to itself through a ternary (`x = changed ? next : x`, `original = isOwn ? original : level`): it writes on every call. Assign only on a change with `if (changed) { x = next }`; this is a conditional write, not the guard that rule 31 forbids in place of a ternary. Fill an unset value with a logical assignment (`original ??= effort`, also `||=` and `&&=`) instead of `x = x ?? y`. Enforce this with oxlint/ESLint `logical-assignment-operators: ["error", "always", { "enforceForIfStatements": true }]`; the self-assigning ternary has no lint rule and is found in review.
    Give a condition with three or more operands, or one that mixes `&&` and `||`, its own named predicate (`const inheritsSessionEffort = (agentId: string | undefined): boolean => agentId !== undefined || !runtime.manager.enabled`) whose name states what it means, and keep only the decision at the use site (`inheritsCompactionLevel(effort) && inheritsSessionEffort(agentId) ? … : undefined`). Branch coverage reports every operand as a branch; small predicates make each uncovered operand easy to find and to cover with one test. When the caller relies on the condition to narrow a type, make the predicate a type guard (`value is T`) or keep that single check inline next to the call.
33. Always ban nesting or chaining ternary operators, including a ternary inside either branch of another. Split the decision into a named helper or a guard. Enforce this mechanically: oxlint/ESLint `no-nested-ternary: "error"` or Biome `style/noNestedTernary: "error"`.
34. Prefer `Array.prototype.map` over `for`.
35. Avoid double or deeper `for` nesting inside a single function scope as much as possible.
36. Replace repeated `if` / `if else` or `switch` with a `Map` object when that replacement is possible.
37. Define functions passed to `sort` separately.
38. Do not add a statement only to destructure a value (for example `const [first] = items;`). Destructure in the parameter list or use the value where it is needed. When a function reads two or more fields of an object parameter, destructure them in the parameter list (`function toolSignature({ text, input, isError }: CallRecord): string`) instead of repeating `param.field` or copying fields into locals; keep the whole parameter only when it is also passed on. Do not put default values in the pattern (rule 47).
    Also keep the whole parameter when a field is assigned through it (`target.input += input`), when its methods need `this` (class instances such as `controller` or `pi`, and string methods), or when an API requires a plain name (a Claude Code mod's `$`, which the validator requires spelled `$.noun.method(…)` at each call). Rename in the pattern when a field would shadow an outer name (`{ contextTokens: tokens }`). Replace a statement that destructures an awaited result (`const { context } = await $.session.usage(…)`) by passing the result on: destructure in the parameter of the `.then` callback or of the function that receives it.
    Do not write a wrapper whose only job is one call on a capability object (`$`, a client, a host API) and that has a single caller (`const setSessionEffort = ($, level) => $.command.run(…)`, `prefetchSession($, generation)`): make the call where it is needed, since the caller already holds the object, and pass the result to a function that takes plain data (`$.session.usage(…).then(({ context }) => prefetchIfCurrent(generation, context))`). Such functions are easier to test, since they need no fake engine.
39. Do not abbreviate names: write `element`, not `el`; `page` or `document`, not `doc`; `englishPhrases`, not `enPhrases`. Established acronyms and units (`id`, `url`, `css`, `dom`, `ms`, `px`) and names required by an external API are allowed. A name only shown in an API's documentation or examples is a convention, not a requirement: write `engine` for a Claude Code mod's `$` (the validator only needs a plain parameter name, spelled `engine.noun.method(…)` at each call), `event` for `e`, `context` for `ctx`, and descriptive type parameters (`ToolResult`, `Value`) for single letters such as `R` or `T`. Members of an external API (`vi.fn`, `event.args`) keep their names. When a rename makes a name collide, rename the inner one to say what it is (`requestContext`).
40. Define a named function whose body is only a `return` of one expression as a `const` arrow function with an explicit return type and a concise body (`const name = (value: Input): Output => expression`), omitting `return`. Define every other named function with `function`, not `const name = () => { … }`. Place these `const` functions with the function definitions, before their first use at module evaluation. Enforce this with the linter (`func-style: ["error", "declaration", { "allowArrowFunctions": true }]` in oxlint or ESLint).

## Values, I/O, and modules

41. Define fixed values as constants.
42. Reduce magic numbers as much as possible.
43. Do not define constants inside a function scope when possible.
44. Keep `const` constants at the top of the file. Ban defining constants between function definitions.
45. Keep `type`, `interface`, and constant definitions at the top of the file.
46. Always ban `let`.
47. Ban including default values in code.
48. Delete unused functions and variables.
49. Delete all unused definitions.
50. Always either delete or use unused constants and variables. Ban fixing them with an underscore prefix. Underscores are allowed only when adjusting function parameters.
51. Minimize file reads and writes. When there are multiple writes, write them in parallel.
52. Escape `\n` correctly when writing it to a file as a string.
53. Ban loading libraries with `import * as`.
54. Always ban creating `index.ts` barrel files.
55. A source directory holds either only files or only subdirectories, never both; split mixed content into sibling subdirectories (for example `detection/state/` and `detection/phrases/`). Keep sibling directories MECE: each has one distinct responsibility, they do not overlap, and together they cover the whole parent. The project root, whose files are placed by tools such as `package.json` or `manifest.json`, is exempt.
56. Always output error logs in English.
57. Always write comments and logs in English.

## External data and DOM

58. Prefer `fetch` for fetching HTML.
59. When fetching data from HTML, always define the processing from actual HTML information.
60. When the data source is not UTF-8, always decode it appropriately with iconv-lite.
61. Never use jsdom. Prefer happy-dom.

## Tests

62. Create a `*.test.ts` test file in the same directory as the file.
63. When creating, editing, changing, or deleting a non-test `.ts` file, always update the related test files.
64. When TypeScript definitions change, automatically update the related tests and keep coverage at 80% or higher.
65. Keep coverage at 90% or higher.
66. Always keep tests for the target file at 90% coverage or higher.
67. Always create test files and keep coverage for the created files at 80% or higher.
68. Keep coverage for the target file at 80% or higher.
69. Do not write test code in a DRY way.
70. Ban `for` in test code. Define test code as NOT DRY as possible.
71. Ban asserting inside a `for` scope in test files.
72. Minimize `describe` usage and always minimize nesting in test code too. Do not overuse `describe`. Reduce `describe` as much as possible.
73. Use `toStrictEqual` instead of `toEqual`.
74. Ban `toContain` assertions.
75. Always ban `expect(.includes(xxx)).toBe(true)` assertions in all test code.
76. Ban variables, constants, and string interpolation in `toBe` and `toStrictEqual` assertion arguments. When asserting strings or arrays, always assert with fixed strings or arrays.
77. Always ban `from bun:test` in test code.
78. In test code, mock all file reads/writes and web requests. Make tests run quickly.
79. Implement tests with execution speed as a priority as well.

## Compiler performance

Based on the TypeScript wiki "Performance" (https://github.com/microsoft/TypeScript/wiki/Performance).

80. Prefer `interface X extends A, B` over intersection type aliases (`A & B`); the compiler caches interface relationships and checks them faster.
81. Annotate the return type of every exported function and of any function whose return type is large or complex; inferring it costs the compiler work on every use.
82. Avoid unions with more than about a dozen members; model them as a base type with subtypes.
83. Give complex conditional and mapped types a named type alias instead of writing them inline in a return position, so the compiler can cache them.
84. In `tsconfig.json`, list only source folders in `include`; never pull in `node_modules`, build output or dot-folders (exclude `**/node_modules` and `**/.*/` when a glob could reach them). Set `types` explicitly, `[]` when no global types are needed.
85. Enable `incremental` for repeated `tsc` runs, with `tsBuildInfoFile` under an ignored folder such as `node_modules/.cache`.
86. Enable `verbatimModuleSyntax` (or `isolatedModules`) when files are transpiled one at a time (Bun, esbuild, vitest, a plugin engine), and import types with `import type`. Ban `const enum` and `namespace`.
87. Keep `strict` on; it includes `strictFunctionTypes`, which makes variance checks faster. Use `skipLibCheck` only for build speed, never to hide declaration errors.
88. When type-checking is slow, measure before changing code: `tsc --extendedDiagnostics`, `--listFilesOnly`, `--explainFiles` and `--generateTrace`.
89. Split a large workspace into evenly sized project references (about 5 to 20), with tests in their own project so product code does not depend on them.
90. When a trace shows a call spending its time inferring a library's generic overload over a large union, pass the type argument explicitly (for example `next<string>(e)`) instead of rewriting or wrapping the library's types. Measure before and after.

## Runtime performance and memory

91. Use the lightest operation that does the job. To replace live state, reassign the reference (`runtime.manager = fresh(…)`) or assign the property; do not copy fields with `Object.assign`. Use spread only when a new object is required, and never `Object.assign({}, value)` (`prefer-object-spread`).
    When two or more fields always change together (`sessionLevel` and `pending`), hold them in one `readonly` record and replace it with a single assignment (`runtime.manager.session = { level, pending: false }`), not one assignment per field: the fields cannot drift apart and each update is one statement. On a hot path, replace the record only when a field changed (`if (effort !== session.level) { session = { level: effort, pending: session.pending } }`) so the common case allocates and writes nothing. When fields change at different rates, split them by rate instead of making everything immutable: state updated once per round (a recovery floor) becomes a `readonly` record replaced in one assignment, while a tally updated on every call stays a small mutable record changed in place and is replaced only when it is reset and not already empty.
92. Ban quadratic patterns: spreading or `concat` into an accumulator inside a loop or `reduce` (`oxc/no-accumulating-spread`), `Array#reduce` (`unicorn/no-array-reduce`), `includes` on a fixed collection (use a `Set` or `Map`: `unicorn/prefer-set-has`), `filter()[0]` or `filter().length > 0` (`find` / `some`: `unicorn/prefer-array-find`, `unicorn/prefer-array-some`) and `map().flat()` (`flatMap`: `unicorn/prefer-array-flat-map`). Await independent work concurrently, never one by one in a loop (`no-await-in-loop`), choosing `Promise.all` or `Promise.allSettled` by rule 98.
93. On hot paths (per step, per tool call, per message) avoid intermediate arrays and copies: no `map` + `includes`, `Object.entries` + `fromEntries`, `[…].join` to build a short string, or `toReversed()` / `[...text]` copies when an index loop, a generator or a template literal does the job. Bound input before serializing it: condense long strings in a `JSON.stringify` replacer rather than stringifying first and slicing after.
94. Bound every structure that grows with session length: a fixed-size ring, a capped `Map`, or an evicting cache. Store a bounded key (length plus head and tail) instead of whole inputs or outputs. On a reset, replace the holder object so the old state is released at once.
95. Enable these mechanically in every package: the oxlint `perf` category plus the rules named in 91 and 92, `complexity: ["error", 10]` and `max-depth: ["error", 3]`. When order or immutability requires an exception (deltas that must arrive in order, a defensive copy of stored state), suppress only that line with `// oxlint-disable-next-line <rule> -- <reason>`.
96. Make complexity and memory regressions fail a test. For each hot path: (a) after a warm-up run, time n and 10n and assert the ratio stays under a linear bound (for example `< 25`), which catches O(n²) without depending on machine speed; (b) force GC (`setFlagsFromString('--expose-gc')` from `node:v8`, then `gc()`), run many iterations and assert `getHeapStatistics().used_heap_size` grows less than a fixed bound; (c) assert size bounds on retained structures and outputs (`Buffer.byteLength(output) <= limit`). Compare ratios and bounds, never absolute milliseconds.

## Errors

97. Never swallow an error. Ban a `catch` block, a `.catch` handler or a `.then` rejection handler that discards the error, returns a stand-in value (`() => undefined`, `() => ({})`, `() => []`, `() => false`) or carries on as if nothing failed. Let the error propagate to the caller or to the host, which reports it (a Claude Code mod's hook that rejects is reported by the engine and skipped, so the session runs as if the hook were not there). Catch only where the code can act on the failure, and then both recover explicitly and report the error with what failed (`reportFailure(engine, 'saving the switch')` logging `dynamic-effort: saving the switch failed: <message>`), or rethrow it with context (`throw new Error('reading the settings failed', { cause: error })`). A detached promise that no caller awaits (`void work()`) ends with such a reporting handler, never an empty one. No lint rule finds a handler that returns a stand-in value, so check every `catch` in review. Tests assert the failure: `await expect(promise).rejects.toThrow('…')` for propagation, the reported text for a handled failure.
98. Choose between `Promise.all` and `Promise.allSettled` by what one failure means. Use `Promise.allSettled` when the tasks are independent and each must run to the end whatever the others do: cleanup and pruning (deleting old keys), fan-out writes, notifications, and independent startup steps (resetting state and registering a command). `Promise.all` rejects at the first failure and leaves the rest unobserved, so one failure would stop or hide the others. Keep `Promise.all` only where the tasks feed one result and a single failure makes the others useless, so failing fast is right (reading inputs that are combined into one value). After `Promise.allSettled`, never ignore a `rejected` result (rule 97): rethrow all failures together (`throw new AggregateError(failures, '<what> failed: …')`) or report each one. Tests assert that a failure in one task leaves the others completed and that the failure is propagated or reported.
