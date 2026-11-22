import Foundation
import MLX
import Testing

@testable import PlamoCore

/// The tiny model plus the committed mini tokenizer in one directory, which is the layout
/// `ModelRuntime` expects: `config.json`, `model.safetensors` and the blob with its `.json` meta.
private func runtime(_ name: String) throws -> ModelRuntime {
    let directory = try Fixtures.temporaryDirectory(name)
    try TinyModel.write(to: directory)
    for file in ["mini-tokenizer.bin", "mini-tokenizer.json"] {
        try FileManager.default.copyItem(
            at: Fixtures.directory.appendingPathComponent(file),
            to: directory.appendingPathComponent(file))
    }
    return try ModelRuntime(
        modelDirectory: directory,
        tokenizerBlob: directory.appendingPathComponent("mini-tokenizer.bin"))
}

@Test func warmUpRunsOneForwardPass() async throws {
    let model = try runtime("runtime-warm")
    let seconds = try await model.warmUp()
    #expect(seconds >= 0)
}

@Test func completesAPromptWithTheTinyTokenizer() async throws {
    let model = try runtime("runtime-complete")
    let completion = try await model.complete(
        CompletionRequest(prompt: "hello", maxTokens: 3, stop: []))
    #expect(completion.promptTokens > 0)
    #expect(completion.tokens <= 3)
    #expect(completion.seconds >= 0)
}

@Test func trimsTheCompletionAtTheFirstStopMarker() async throws {
    let model = try runtime("runtime-stop")
    let completion = try await model.complete(
        CompletionRequest(prompt: "hello", maxTokens: 3, stop: ["", " "]))
    #expect(!completion.text.contains(" "))
}

@Test func encodesAndDecodesTheSameText() async throws {
    let model = try runtime("runtime-encode")
    let ids = await model.encode("hello")
    #expect(!ids.isEmpty)
    // Decoding drops the boundary markers, so it is not a byte-for-byte round trip.
    let decoded = try await model.decode(ids)
    #expect(!decoded.isEmpty)
    let eos = try TinyModel.config().eosTokenId
    #expect(await model.eosTokenIDs == eos)
}

@Test func debugStagesCoverEveryLayer() async throws {
    let model = try runtime("runtime-stages")
    let result = try await model.stages(prompt: "hello")
    let vocabSize = try TinyModel.config().vocabSize
    #expect(!result.ids.isEmpty)
    #expect(result.top.count == vocabSize)
    #expect(result.stages.contains { $0.0 == "embed" })
    #expect(result.stages.contains { $0.0 == "L1.mlp" })
    #expect(result.stages.allSatisfy { !$0.1.isEmpty })
}

@Test func timingReportsBothPhases() async throws {
    let model = try runtime("runtime-timing")
    let prefill = try await model.timePrefill([2, 3, 4])
    #expect(prefill.tokens == 3)
    #expect(prefill.build >= 0 && prefill.eval >= 0)
    let decode = try await model.timeDecode(prompt: [2, 3], steps: 2)
    #expect(decode.tokens == 2)
}

@Test func theMemoryLimitIsConfigurableAndReleasable() {
    MLXMemory.configure(cacheLimitBytes: 4 * 1_024 * 1_024)
    #expect(Memory.cacheLimit == 4 * 1_024 * 1_024)
    MLXMemory.release()
    MLXMemory.configure()
    #expect(Memory.cacheLimit == MLXMemory.defaultCacheLimitBytes)
}
