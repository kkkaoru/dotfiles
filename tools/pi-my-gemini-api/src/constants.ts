// This file runs with bun.

export const PROVIDER_ID = "my-gemini-api";
export const PROVIDER_NAME = "Gemini API (my)";
export const API_ID = "my-gemini-api";

/** The official CLI this provider runs instead of speaking the Cloud Code Assist wire itself. */
export const CLI_NAME = "gemini-api";
export const CLI_REPO = "https://github.com/google-gemini/gemini-api-cli";
export const INSTALL_HINT = `Install ${CLI_NAME} (${CLI_REPO}) or set GEMINI_API_CLI to its path.`;

/** The CLI's own default model, used until `models list` reports the account's catalog. */
export const DEFAULT_MODEL = "gemini-4-hydrogen";

/** pi requires a baseUrl once a provider declares models; the CLI owns the real endpoint. */
export const DEFAULT_ENDPOINT = "https://generativelanguage.googleapis.com";

/** A stuck child process must not hang the agent loop. */
export const CLI_TIMEOUT_MS = 600_000;
