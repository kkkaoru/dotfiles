import Dispatch
import Foundation
import MLX

/// MLX's Metal buffer cache. The default limit is the device's recommended maximum, so a burst of
/// prefills leaves hundreds of MB the server never asks for again.
public enum MLXMemory {
    /// 32 MB: enough for one decode step's buffers, far below the device maximum.
    public static let defaultCacheLimitBytes = 32 * 1_024 * 1_024

    /// Caps the Metal buffer cache so a burst of prefills does not stay resident.
    public static func configure(cacheLimitBytes: Int = MLXMemory.defaultCacheLimitBytes) {
        Memory.cacheLimit = cacheLimitBytes
    }

    /// Returns the cached buffers to the system.
    public static func release() {
        Memory.clearCache()
    }
}

/// One debug forward pass: the per-stage norms, the final logits and the prompt ids.
public struct StageTrace: Sendable {
    public let stages: [(String, [Float])]
    public let top: [Float]
    public let ids: [Int32]
}

/// The model and its tokenizer, owned by one actor that runs on its own serial queue: a decode step
/// blocks that queue instead of a cooperative-pool thread, and only one decode runs at a time.
public actor ModelRuntime: CompletionProviding {
    private let model: PlamoModel
    private let tokenizer: PlamoTokenizer
    private let queue = DispatchSerialQueue(label: "plamo-swift.model")

    nonisolated public var unownedExecutor: UnownedSerialExecutor {
        queue.asUnownedSerialExecutor()
    }

    public init(modelDirectory: URL, tokenizerBlob: URL) throws {
        let meta = tokenizerBlob.deletingPathExtension().appendingPathExtension("json")
        // Ask the kernel for the weights before the tokenizer decode runs: a cold start reads
        // 478 MB from disk, and the tokenizer work is free cover for the first part of that read.
        Prefetch.weights(in: modelDirectory)
        tokenizer = try PlamoTokenizer(blob: tokenizerBlob, meta: meta)
        model = try PlamoModel.load(directory: modelDirectory)
    }

    /// One forward pass compiles the Metal kernels; it is what keeps the first completion cheap.
    /// The prompt is request-shaped because a single token leaves the multi-token prefill path
    /// cold: measured 11.1 ms on the first request against 4.3–5.8 ms once that shape is warm.
    /// Returns the seconds the warm-up took.
    public func warmUp() throws -> Double {
        let started = Date()
        let ids = tokenizer.promptIDs(ModelRuntime.warmPrompt, bos: model.config.bosTokenId)
        var logits = try model.logits(ids: ids)
        MLX.eval(logits)
        // The template prefill leaves the single-token decode step cold, and every request runs it:
        // measured 26.3 ms for the first request against 19.1 ms for the next one in the same
        // process. One decode step here moves that cost into the load the caller already waits for.
        let next = logits.argMax(axis: -1).item(Int32.self)
        logits = try model.logits(ids: [next])
        MLX.eval(logits)
        model.resetCache()
        MLXMemory.release()
        return Date().timeIntervalSince(started)
    }

    /// The translation template the mods send, which is what the first completion actually looks
    /// like; its token count is what makes the warm-up touch the prefill shapes.
    private static let warmPrompt =
        "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>こんにちは、世界。"
        + "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>"

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
    public func stages(prompt: String) throws -> StageTrace {
        var captured = [(String, [Float])]()
        let ids = tokenizer.promptIDs(prompt, bos: model.config.bosTokenId)
        let scores = try model.logits(ids: ids) { stage, hidden in
            let last = hidden[0..., (hidden.dim(1) - 1)..<hidden.dim(1), 0...]
            captured.append((stage, last.asArray(Float.self)))
        }
        return StageTrace(stages: captured, top: scores.asArray(Float.self), ids: ids)
    }

    public var eosTokenIDs: [Int32] {
        model.config.eosTokenId
    }
}
