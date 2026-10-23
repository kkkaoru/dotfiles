// @vitest-environment node
// Runs with bun: `bun run test` (vitest).
import stylelint from "stylelint";
import { expect, test } from "vitest";
import plugins, { LITERAL_TOKEN, NO_MIXED } from "./custom-properties.ts";

const lintTexts = async (code: string, rules: Record<string, unknown>): Promise<string[]> => {
  const { results } = await stylelint.lint({ code, config: { plugins, rules } });
  return results.flatMap((result) => result.warnings.map((warning) => warning.text));
};

test("no-mixed-custom-properties accepts variable-only and style-only rules", async () => {
  expect(
    await lintTexts(":host { --size: 1px; } .frame { --color: red; } .label { top: 0; }", {
      [NO_MIXED]: true,
    }),
  ).toStrictEqual([]);
});

test("no-mixed-custom-properties rejects a rule that declares variables and styles", async () => {
  expect(await lintTexts(".frame { --color: red; top: 0; }", { [NO_MIXED]: true })).toStrictEqual([
    'Expected ".frame" to declare only custom properties or only styles (local/no-mixed-custom-properties)',
  ]);
});

test("no-mixed-custom-properties ignores rules without declarations", async () => {
  expect(await lintTexts(".frame {}", { [NO_MIXED]: true })).toStrictEqual([]);
});

test("literal-token-custom-properties accepts literal token values and var() elsewhere", async () => {
  expect(
    await lintTexts(
      ":host { --size: 1px; } .frame { --color: var(--size); } .other { --x: var(--size); }",
      {
        [LITERAL_TOKEN]: ":host",
      },
    ),
  ).toStrictEqual([]);
});

test("literal-token-custom-properties rejects var() in the token scope, including inside @media", async () => {
  expect(
    await lintTexts(
      ":host { --a: 1px; --b: var(--a); } @media (prefers-reduced-motion: reduce) { :host { --c: var(--a); } }",
      { [LITERAL_TOKEN]: ":host" },
    ),
  ).toStrictEqual([
    'Expected ":host" custom property "--b" to have a literal value without var() (local/literal-token-custom-properties)',
    'Expected ":host" custom property "--c" to have a literal value without var() (local/literal-token-custom-properties)',
  ]);
});

test("both rules reject invalid options", async () => {
  const { results } = await stylelint.lint({
    code: ".frame { top: 0; }",
    config: { plugins, rules: { [NO_MIXED]: "yes", [LITERAL_TOKEN]: true } },
  });
  expect(
    results.flatMap((result) => result.invalidOptionWarnings.map((warning) => warning.text)),
  ).toStrictEqual([
    'Invalid option value "yes" for rule "local/no-mixed-custom-properties"',
    'Invalid option value "true" for rule "local/literal-token-custom-properties"',
  ]);
});
