export default function (pi) {
  pi.registerProvider("compaction-test", {
    baseUrl: process.env.PI_TEST_API_URL,
    apiKey: "local-test-only",
    api: "openai-completions",
    models: [
      {
        id: "test-model",
        name: "Compaction integration test",
        reasoning: false,
        input: ["text"],
        contextWindow: 200000,
        maxTokens: 16384,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      },
    ],
  });
}
