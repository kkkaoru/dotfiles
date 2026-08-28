// This TypeScript file is executed with Bun.
interface FastModel {
  readonly id: string;
  readonly provider: string;
}

const FAST_PROVIDERS = new Set(["openai", "openai-codex", "azure-openai-responses"]);

export function fastEligible(model: FastModel | undefined): boolean {
  return model !== undefined && FAST_PROVIDERS.has(model.provider) && model.id.startsWith("gpt-5");
}
