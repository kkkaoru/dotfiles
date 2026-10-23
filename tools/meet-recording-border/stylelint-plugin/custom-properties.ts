// Runs with bun: loaded by stylelint in `bun run lint` (Node strips the types), tested by `bun run test`.
// Enforces the overlay's variable layout: variable-only scopes, and literal tokens in the token scope.
import type { ChildNode, Declaration, Rule as CssRule, Root } from "postcss";
import stylelint, { type PostcssResult, type Rule, type RuleBase } from "stylelint";

interface ReferenceReport {
  readonly result: PostcssResult;
  readonly selector: string;
  readonly declaration: Declaration;
}

export const NO_MIXED = "local/no-mixed-custom-properties";
export const LITERAL_TOKEN = "local/literal-token-custom-properties";
const CUSTOM_PROPERTY_PREFIX = "--";
const VAR_FUNCTION = /\bvar\(/u;
const META_URL = "https://github.com/kkkaoru/dotfiles/tree/master/tools/meet-recording-border";

const { createPlugin, utils: stylelintUtilities } = stylelint;

const noMixedMessages = stylelintUtilities.ruleMessages(NO_MIXED, {
  rejected: (selector: string) =>
    `Expected "${selector}" to declare only custom properties or only styles`,
});

const literalTokenMessages = stylelintUtilities.ruleMessages(LITERAL_TOKEN, {
  rejected: (selector: string, property: string) =>
    `Expected "${selector}" custom property "${property}" to have a literal value without var()`,
});

const isDeclaration = (node: ChildNode): node is Declaration => node.type === "decl";

const isCustomProperty = (declaration: Declaration): boolean =>
  declaration.prop.startsWith(CUSTOM_PROPERTY_PREFIX);

const declarationsOf = (rule: CssRule): Declaration[] =>
  rule.nodes.filter((node) => isDeclaration(node));

const mixesKinds = (declarations: readonly Declaration[]): boolean =>
  declarations.some((declaration) => isCustomProperty(declaration)) &&
  !declarations.every((declaration) => isCustomProperty(declaration));

const referencesVariable = (declaration: Declaration): boolean =>
  isCustomProperty(declaration) && VAR_FUNCTION.test(declaration.value);

const rulesOf = (root: Root): CssRule[] => {
  const rules: CssRule[] = [];
  root.walkRules((rule) => {
    rules.push(rule);
  });
  return rules;
};

const isSelector = (value: unknown): value is string => typeof value === "string";

const reportMixed = (result: PostcssResult, rule: CssRule): void =>
  stylelintUtilities.report({
    ruleName: NO_MIXED,
    result,
    node: rule,
    message: noMixedMessages.rejected(rule.selector),
  });

const reportReference = ({ result, selector, declaration }: ReferenceReport): void =>
  stylelintUtilities.report({
    ruleName: LITERAL_TOKEN,
    result,
    node: declaration,
    message: literalTokenMessages.rejected(selector, declaration.prop),
  });

const mixedRules = (root: Root): CssRule[] =>
  rulesOf(root).filter((rule) => mixesKinds(declarationsOf(rule)));

// Invalid options report nothing: validateOptions has already reported them.
const noMixed: RuleBase = (primary: unknown) => (root, result) => {
  (stylelintUtilities.validateOptions(result, NO_MIXED, { actual: primary, possible: [true] })
    ? mixedRules(root)
    : []
  ).map((rule) => reportMixed(result, rule));
};

// Returns the token scope's selector when the primary option is a valid one.
const validSelector = (result: PostcssResult, primary: unknown): string | undefined =>
  stylelintUtilities.validateOptions(result, LITERAL_TOKEN, {
    actual: primary,
    possible: [isSelector],
  }) && isSelector(primary)
    ? primary
    : undefined;

// Matches the token scope inside @media as well, since walkRules visits nested rules.
const referencingDeclarations = (root: Root, selector: string): Declaration[] =>
  rulesOf(root)
    .filter((rule) => rule.selector === selector)
    .flatMap((rule) => declarationsOf(rule))
    .filter((declaration) => referencesVariable(declaration));

const literalToken: RuleBase = (primary: unknown) => (root, result) => {
  const selector = validSelector(result, primary);
  if (selector === undefined) {
    return;
  }
  referencingDeclarations(root, selector).map((declaration) =>
    reportReference({ result, selector, declaration }),
  );
};

const noMixedRule: Rule = Object.assign(noMixed, {
  ruleName: NO_MIXED,
  messages: noMixedMessages,
  meta: { url: META_URL },
});

const literalTokenRule: Rule = Object.assign(literalToken, {
  ruleName: LITERAL_TOKEN,
  messages: literalTokenMessages,
  meta: { url: META_URL },
});

export default [createPlugin(NO_MIXED, noMixedRule), createPlugin(LITERAL_TOKEN, literalTokenRule)];
