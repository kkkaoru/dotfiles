import Dispatch
import Foundation
import MLX

/// MLX's Metal buffer cache. The default limit is the device's recommended maximum, so a burst of
/// prefills leaves hundreds of MB the server never asks for again.
public enum MLXMemory {
    public static let defaultCacheLimitBytes = 32 * 1024 * 1024

    public static func configure(cacheLimitBytes: Int = MLXMemory.defaultCacheLimitBytes) {
        Memory.cacheLimit = cacheLimitBytes
    }

    public static func release() {
        Memory.clearCache()
    }
}

/// The model and its tokenizer, owned by one actor that runs on its own serial queue: a decode step
/// blocks that queue instead of a cooperative-pool thread, and only one decode runs at a time.
public actor ModelRuntime: CompletionProviding {
    private let model: PlamoModel
    private let tokenizer: PlamoTokenizer
    private let queue = DispatchSerialQueue(label: "plamo-swift.model")

    public nonisolated var unownedExecutor: UnownedSerialExecutor {
        queue.asUnownedSerialExecutor()
    }

    public init(modelDirectory: URL, tokenizerBlob: URL) throws {
        let meta = tokenizerBlob.deletingPathExtension().appendingPathExtension("json")
        tokenizer = try PlamoTokenizer(blob: tokenizerBlob, meta: meta)
        model = try PlamoModel.load(directory: modelDirectory)
    }

    /// One forward pass compiles the Metal kernels; it is what keeps the first completion cheap.
    /// Returns the seconds the warm-up took.
    public func warmUp() throws -> Double {
        let started = Date()
        try MLX.eval(model.logits(ids: [model.config.bosTokenId]))
        MLXMemory.release()
        return Date().timeIntervalSince(started)
    }

    public func complete(_ request: CompletionRequest) async throws -> Completion {
        let started = Date()
        let promptIDs = tokenizer.promptIDs(request.prompt, bos: model.config.bosTokenId)
        let produced = try model.generate(
            prompt: promptIDs, maxTokens: request.maxTokens,
            stopIDs: Set(model.config.eosTokenId))
        var text = try tokenizer.decode(produced)
        for marker in request.stop where !marker.isEmpty {
            if let range = text.range(of: marker) {
                text = String(text[..<range.lowerBound])
                break
            }
        }
        return Completion(
            text: text, tokens: produced.count, seconds: Date().timeIntervalSince(started))
    }

    public func encode(_ text: String) -> [Int32] {
        tokenizer.promptIDs(text, bos: model.config.bosTokenId)
    }

    public func decode(_ ids: [Int32]) throws -> String {
        try tokenizer.decode(ids)
    }

    /// Stage-by-stage norms for `debug`: the numbers the Python server prints for the same prompt.
    public func stages(
        prompt: String
    ) throws -> (stages: [(String, [Float])], top: [Float], ids: [Int32]) {
        var captured = [(String, [Float])]()
        let ids = tokenizer.promptIDs(prompt, bos: model.config.bosTokenId)
        let scores = try model.logits(ids: ids) { stage, hidden in
            let last = hidden[0..., (hidden.dim(1) - 1)..<hidden.dim(1), 0...]
            captured.append((stage, last.asArray(Float.self)))
        }
        return (captured, scores.asArray(Float.self), ids)
    }

    public var eosTokenIDs: [Int32] {
        model.config.eosTokenId
    }
}
