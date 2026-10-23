// Runs with bun: scripts/css-text-plugin.ts (build) and the vitest css-text plugin import CSS as text.
declare module "*.css" {
  const text: string;
  export default text;
}
