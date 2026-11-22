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
        // Ask the kernel for the weights before the tokenizer decode runs: a cold start reads 478 MB
        // from disk, and the tokenizer work is free cover for the first part of that read.
        Prefetch.weights(in: modelDirectory)
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

    /// One forward pass split into the time the CPU spends building the graph and the time the GPU
    /// spends running it. `bench` uses this to say which side a change actually helped.
    public struct ForwardTiming: Sendable {
        public let tokens: Int
        public let build: Double
        public let eval: Double
    }

    public var layerCount: Int { model.config.numHiddenLayers }
    public var windowSize: Int { model.config.windowSize }

    public func timePrefill(_ ids: [Int32]) throws -> ForwardTiming {
        model.resetCache()
        let buildStarted = Date()
        let logits = try model.logits(ids: ids)
        let build = Date().timeIntervalSince(buildStarted)
        let evalStarted = Date()
        MLX.eval(logits)
        return ForwardTiming(
            tokens: ids.count, build: build, eval: Date().timeIntervalSince(evalStarted))
    }

    /// Decode steps after a prefill of `prompt`, averaged per step.
    public func timeDecode(prompt: [Int32], steps: Int) throws -> ForwardTiming {
        model.resetCache()
        var logits = try model.logits(ids: prompt)
        MLX.eval(logits)
        var build = 0.0
        var eval = 0.0
        for _ in 0..<steps {
            let next = logits.argMax(axis: -1).item(Int32.self)
            let buildStarted = Date()
            logits = try model.logits(ids: [next])
            build += Date().timeIntervalSince(buildStarted)
            let evalStarted = Date()
            MLX.eval(logits)
            eval += Date().timeIntervalSince(evalStarted)
        }
        let count = Double(max(steps, 1))
        return ForwardTiming(tokens: steps, build: build / count, eval: eval / count)
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
            text: text, promptTokens: promptIDs.count, tokens: produced.count,
            seconds: Date().timeIntervalSince(started))
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
