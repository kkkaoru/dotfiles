import Foundation
import MLX

@testable import PlamoCore

/// A two-layer, 8-wide model with the real architecture: small enough to build in memory, complete
/// enough to exercise attention, the sliding window, the mask and greedy decode.
enum TinyModel {
    static let configJSON = """
        {
          "hidden_size": 8,
          "num_hidden_layers": 2,
          "num_attention_heads": 2,
          "num_key_value_heads": 1,
          "head_dim": 4,
          "intermediate_size": 16,
          "vocab_size": 300,
          "rms_norm_eps": 1e-5,
          "window_size": 3,
          "sliding_window_pattern": 2,
          "rope_parameters": {
            "full_attention": {"rope_type": "default", "rope_theta": 1000000},
            "sliding_attention": {"rope_type": "default", "rope_theta": 10000}
          },
          "eos_token_id": [1],
          "bos_token_id": 2
        }
        """

    static func config() throws -> PlamoConfig {
        try JSONDecoder().decode(PlamoConfig.self, from: Data(configJSON.utf8))
    }

    /// Plain float32 weights: the loader's dense path, and no quantization noise in the assertions.
    static func arrays() throws -> [String: MLXArray] {
        let config = try config()
        var arrays: [String: MLXArray] = [
            "model.embed_tokens.weight": dense([config.vocabSize, config.hiddenSize], seed: 1),
            "model.norm.weight": filled([config.hiddenSize], 1),
            "lm_head.weight": dense([config.vocabSize, config.hiddenSize], seed: 2),
        ]
        let hidden = config.hiddenSize
        let queryDim = config.numAttentionHeads * config.headDim
        let keyDim = config.numKeyValueHeads * config.headDim
        for index in 0..<config.numHiddenLayers {
            let base = "model.layers.layers.\(index)"
            arrays["\(base).pre_mixer_norm.weight"] = filled([hidden], 1)
            arrays["\(base).post_mixer_norm.weight"] = filled([hidden], 1)
            arrays["\(base).pre_mlp_norm.weight"] = filled([hidden], 1)
            arrays["\(base).post_mlp_norm.weight"] = filled([hidden], 1)
            arrays["\(base).mixer.q_norm.weight"] = filled([config.headDim], 1)
            arrays["\(base).mixer.k_norm.weight"] = filled([config.headDim], 1)
            arrays["\(base).mixer.qkv_proj.weight"] = dense(
                [queryDim + 2 * keyDim, hidden], seed: 10 + index)
            arrays["\(base).mixer.o_proj.weight"] = dense([hidden, queryDim], seed: 20 + index)
            arrays["\(base).mlp.gate_up_proj.weight"] = dense(
                [2 * config.intermediateSize, hidden], seed: 30 + index)
            arrays["\(base).mlp.down_proj.weight"] = dense(
                [hidden, config.intermediateSize], seed: 40 + index)
        }
        return arrays
    }

    /// A model directory the loader and the runtime can open: config.json plus safetensors.
    static func write(to directory: URL) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data(configJSON.utf8).write(to: directory.appendingPathComponent("config.json"))
        try MLX.save(
            arrays: try arrays(), url: directory.appendingPathComponent("model.safetensors"))
    }

    private static func dense(_ shape: [Int], seed: Int) -> MLXArray {
        let count = shape.reduce(1, *)
        let values = (0..<count).map { index -> Float in
            Float((index &* 31 &+ seed &* 17) % 23) / 23 - 0.5
        }
        return MLXArray(values).reshaped(shape)
    }

    private static func filled(_ shape: [Int], _ value: Float) -> MLXArray {
        MLXArray([Float](repeating: value, count: shape.reduce(1, *))).reshaped(shape)
    }
}

/// The committed tokenizer fixture, and the paths the tests open.
enum Fixtures {
    static let directory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Fixtures")

    static var tokenizerBlob: URL { directory.appendingPathComponent("mini-tokenizer.bin") }

    static var tokenizerMeta: URL { directory.appendingPathComponent("mini-tokenizer.json") }

    static func tokenizer() throws -> PlamoTokenizer {
        try PlamoTokenizer(blob: tokenizerBlob, meta: tokenizerMeta)
    }

    /// A unique temporary directory, removed when the process exits.
    static func temporaryDirectory(_ name: String) throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("plamo-swift-tests-\(name)-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}

/// A completion provider the router and server tests control: it records the request it saw.
struct StubProvider: CompletionProviding {
    var completion = Completion(text: "hello", tokens: 3, seconds: 0.01)
    var failure: PlamoError?
    var recorder: Recorder?

    func complete(_ request: CompletionRequest) async throws -> Completion {
        await recorder?.record(request)
        if let failure { throw failure }
        return completion
    }
}

actor Recorder {
    private(set) var requests: [CompletionRequest] = []

    func record(_ request: CompletionRequest) {
        requests.append(request)
    }
}
