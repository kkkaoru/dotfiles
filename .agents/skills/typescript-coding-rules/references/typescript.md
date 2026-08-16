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

23. Prefer splitting functions, normalizing them, and writing DRY code.
24. Keep non-test code as DRY as possible.
25. When a `main` or `run` function exists, split the work so that these functions have as few steps as possible.
26. When creating or defining a `main` function, reduce definitions inside the `main` function scope as much as possible, split function definitions, and make coverage easier to raise.
27. When a function has 3 or more parameters, take one object as the argument.
28. Use sugar syntax to omit `return` when it can be omitted.
29. Minimize `return` and `const` definitions within a readable range.
30. Minimize nesting in all code.
31. Use a guard to simplify nested logic when possible. Ban replacing a ternary operator with a guard.
32. Use a ternary operator whenever possible.
33. Ban nesting or chaining ternary operators.
34. Prefer `Array.prototype.map` over `for`.
35. Avoid double or deeper `for` nesting inside a single function scope as much as possible.
36. Replace repeated `if` / `if else` or `switch` with a `Map` object when that replacement is possible.
37. Define functions passed to `sort` separately.

## Values, I/O, and modules

38. Define fixed values as constants.
39. Reduce magic numbers as much as possible.
40. Do not define constants inside a function scope when possible.
41. Keep `const` constants at the top of the file. Ban defining constants between function definitions.
42. Keep `type`, `interface`, and constant definitions at the top of the file.
43. Always ban `let`.
44. Ban including default values in code.
45. Delete unused functions and variables.
46. Delete all unused definitions.
47. Always either delete or use unused constants and variables. Ban fixing them with an underscore prefix. Underscores are allowed only when adjusting function parameters.
48. Minimize file reads and writes. When there are multiple writes, write them in parallel.
49. Escape `\n` correctly when writing it to a file as a string.
50. Ban loading libraries with `import * as`.
51. Always ban creating `index.ts` barrel files.
52. Always output error logs in English.
53. Always write comments and logs in English.

## External data and DOM

54. Prefer `fetch` for fetching HTML.
55. When fetching data from HTML, always define the processing from actual HTML information.
56. When the data source is not UTF-8, always decode it appropriately with iconv-lite.
57. Never use jsdom. Prefer happy-dom.

## Tests

58. Create a `*.test.ts` test file in the same directory as the file.
59. When creating, editing, changing, or deleting a non-test `.ts` file, always update the related test files.
60. When TypeScript definitions change, automatically update the related tests and keep coverage at 80% or higher.
61. Keep coverage at 90% or higher.
62. Always keep tests for the target file at 90% coverage or higher.
63. Always create test files and keep coverage for the created files at 80% or higher.
64. Keep coverage for the target file at 80% or higher.
65. Do not write test code in a DRY way.
66. Ban `for` in test code. Define test code as NOT DRY as possible.
67. Ban asserting inside a `for` scope in test files.
68. Minimize `describe` usage and always minimize nesting in test code too. Do not overuse `describe`. Reduce `describe` as much as possible.
69. Use `toStrictEqual` instead of `toEqual`.
70. Ban `toContain` assertions.
71. Always ban `expect(.includes(xxx)).toBe(true)` assertions in all test code.
72. Ban variables, constants, and string interpolation in `toBe` and `toStrictEqual` assertion arguments. When asserting strings or arrays, always assert with fixed strings or arrays.
73. Always ban `from bun:test` in test code.
74. In test code, mock all file reads/writes and web requests. Make tests run quickly.
75. Implement tests with execution speed as a priority as well.
