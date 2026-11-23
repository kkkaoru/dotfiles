import Foundation
import MLX

/// The two mask tables a prefill builds: full attention and the sliding window.
private typealias MaskPair = (full: MLXArray?, sliding: MLXArray?)

/// PLaMo 3 in MLX Swift: the same weights, the same arithmetic, and the KV cache trimmed to the
/// sliding window so a long conversation does not grow without bound.

public final class PlamoModel {
    /// One transformer block's weights, already in the activation dtype.
    public struct Layer {
        // Norm weights carry their offsets, baked in at load in the activation dtype.
        internal let preMixerNorm: MLXArray
        internal let postMixerNorm: MLXArray
        internal let preMLPNorm: MLXArray
        internal let postMLPNorm: MLXArray
        internal let qNorm: MLXArray
        internal let kNorm: MLXArray
        internal let qkv: Quant
        internal let oProj: Quant
        internal let gateUp: Quant
        internal let downProj: Quant
        internal let fullAttention: Bool
        internal let rotary: Rotary
    }

    private struct Cache {
        var keys: MLXArray
        var values: MLXArray
    }

    /// The checkpoint's shape.
    public let config: PlamoConfig
    private let layers: [Layer]
    private let embedTokens: Quant
    private let finalNorm: MLXArray
    private let lmHead: Quant
    private var caches: [Cache?]
    private var seen = 0
    /// The packed cache these weights were adopted from, so its pages can be returned once they are
    /// no longer pointed into.
    public let mapping: Packed.Mapping?

    /// Loads `config.json` and `model.safetensors` from `directory`. A packed cache beside the
    /// checkpoint is adopted without copying; otherwise MLX's own loader reads the checkpoint.
    public static func load(directory: URL) throws -> PlamoModel {
        let config = try PlamoConfig.load(directory: directory)
        let url = directory.appendingPathComponent("model.safetensors")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw PlamoError.missingModelFile("model.safetensors")
        }
        if let packed = Packed.loadCurrent(checkpoint: url) {
            Timing.mark("reload.packed")
            return try PlamoModel.make(
                config: config, arrays: packed.arrays, mapping: packed.mapping)
        }
        let arrays = try loadArrays(url: url)
        Timing.mark("reload.arrays")
        return try PlamoModel.make(config: config, arrays: arrays)
    }

    /// Builds the model from named arrays: the checkpoint loader and the tests' tiny fixtures
    /// differ only in where the arrays come from. A missing tensor is reported by name.
    public static func make(
        config: PlamoConfig, arrays: [String: MLXArray], mapping: Packed.Mapping? = nil
    ) throws -> PlamoModel {
        try config.validate()
        func array(_ name: String) throws -> MLXArray {
            guard let value = arrays[name] else { throw PlamoError.missingModelFile(name) }
            return value
        }
        func shifted(_ name: String, _ offset: Float, _ dtype: DType) throws -> MLXArray {
            (try array(name) + offset).asType(dtype)
        }

        let embedTokens = try Quant(arrays: arrays, prefix: "model.embed_tokens")
        let dtype = embedTokens.activationDType
        guard let fullParameters = config.ropeParameters["full_attention"],
            let slidingParameters = config.ropeParameters["sliding_attention"]
        else {
            throw PlamoError.invalidConfig(field: "rope_parameters")
        }
        let full = Rotary(fullParameters, headDim: config.headDim)
        let sliding = Rotary(slidingParameters, headDim: config.headDim)

        var layers = [Layer]()
        layers.reserveCapacity(config.numHiddenLayers)
        for index in 0..<config.numHiddenLayers {
            let base = "model.layers.layers.\(index)"
            let fullAttention = (index + 1).isMultiple(of: config.slidingWindowPattern)
            layers.append(
                Layer(
                    preMixerNorm: try shifted("\(base).pre_mixer_norm.weight", 1, dtype),
                    postMixerNorm: try shifted("\(base).post_mixer_norm.weight", 1.0 / 5, dtype),
                    preMLPNorm: try shifted("\(base).pre_mlp_norm.weight", 1, dtype),
                    postMLPNorm: try shifted(
                        "\(base).post_mlp_norm.weight", 1 / pow(Float(5), 1.5), dtype),
                    qNorm: try shifted("\(base).mixer.q_norm.weight", 1, dtype),
                    kNorm: try shifted("\(base).mixer.k_norm.weight", 1, dtype),
                    qkv: try Quant(arrays: arrays, prefix: "\(base).mixer.qkv_proj"),
                    oProj: try Quant(arrays: arrays, prefix: "\(base).mixer.o_proj"),
                    gateUp: try Quant(arrays: arrays, prefix: "\(base).mlp.gate_up_proj"),
                    downProj: try Quant(arrays: arrays, prefix: "\(base).mlp.down_proj"),
                    fullAttention: fullAttention,
                    rotary: fullAttention ? full : sliding))
        }

        return PlamoModel(
            config: config,
            layers: layers,
            embedTokens: embedTokens,
            finalNorm: try shifted("model.norm.weight", 1, dtype),
            lmHead: try Quant(arrays: arrays, prefix: "lm_head"),
            mapping: mapping)
    }

    private init(
        config: PlamoConfig, layers: [Layer], embedTokens: Quant, finalNorm: MLXArray,
        lmHead: Quant,
        mapping: Packed.Mapping?
    ) {
        self.config = config
        self.mapping = mapping
        self.layers = layers
        self.embedTokens = embedTokens
        self.finalNorm = finalNorm
        self.lmHead = lmHead
        self.caches = Array(repeating: nil, count: config.numHiddenLayers)
    }

    /// Drops the KV cache, so the next call prefills from scratch.
    public func resetCache() {
        caches = Array(repeating: nil, count: config.numHiddenLayers)
        seen = 0
    }

    /// Positions the cache has consumed; the next token's RoPE positions start here.
    public var position: Int { seen }

    /// logits of the last position, and the KV cache grows by `ids.count` tokens.
    public func logits(
        ids: [Int32], onStage: ((String, MLXArray) -> Void)? = nil
    ) throws -> MLXArray {
        guard !ids.isEmpty else { throw PlamoError.invalidRequest(reason: "empty token list") }
        let length = ids.count
        let hidden = embedTokens.embed(MLXArray(ids)).reshaped([1, length, config.hiddenSize])
        return try forward(hidden, length: length, onStage: onStage)
    }

    /// The same pass for one token given as an array: a decode loop can feed the previous step's
    /// `argMax` straight back in and keep the whole chain on the GPU until it needs the values.
    public func logits(token: MLXArray) throws -> MLXArray {
        let hidden = embedTokens.embed(token).reshaped([1, 1, config.hiddenSize])
        return try forward(hidden, length: 1, onStage: nil)
    }

    private func forward(
        _ embedded: MLXArray, length: Int, onStage: ((String, MLXArray) -> Void)?
    ) throws -> MLXArray {
        var hidden = embedded
        onStage?("embed", hidden)
        let start = seen
        let queryPositions = (start..<(start + length)).map { $0 }
        // One table per attention kind instead of one per layer: every layer used to rebuild the
        // same [queries][keys] values, which is 16 identical allocations on every prefill.
        let masks = try prefillMasks(queries: length, start: start)

        for (index, layer) in layers.enumerated() {
            let residual = hidden
            var h = norm(hidden, layer.preMixerNorm)
            onStage?("L\(index).pre_mixer", h)
            h = try attention(
                h, layer: layer, index: index, positions: queryPositions,
                mask: layer.fullAttention ? masks.full : masks.sliding)
            onStage?("L\(index).mixer", h)
            hidden = residual + norm(h, layer.postMixerNorm)

            let residualMLP = hidden
            h = norm(hidden, layer.preMLPNorm)
            h = mlp(h, layer: layer)
            onStage?("L\(index).mlp", h)
            hidden = residualMLP + norm(h, layer.postMLPNorm)
        }
        seen += length

        hidden = norm(hidden, finalNorm)
        let last = hidden[0..., (length - 1)..<length, 0...]
        return lmHead.matmul(last)
    }

    /// The masks a prefill needs. Full attention sees every cached key; a sliding layer holds at
    /// most `windowSize`, so the whole pass needs at most these two tables.
    private func prefillMasks(queries: Int, start: Int) throws -> MaskPair {
        guard queries > 1 else { return (nil, nil) }
        let dtype = embedTokens.activationDType
        let total = start + queries
        func table(keys: Int, window: Int?) throws -> MLXArray {
            let values = try Mask.causal(queries: queries, keys: keys, window: window)
            // The mask has to carry the activation dtype: the model runs in bfloat16 and MLX will
            // not promote a float32 mask into it.
            return MLXArray(values).reshaped([1, 1, queries, keys]).asType(dtype)
        }
        return (
            try table(keys: total, window: nil),
            try table(keys: min(total, config.windowSize), window: config.windowSize)
        )
    }

    private func norm(_ x: MLXArray, _ weight: MLXArray) -> MLXArray {
        MLXFast.rmsNorm(x, weight: weight, eps: config.rmsNormEps)
    }

    private func attention(
        _ x: MLXArray, layer: Layer, index: Int, positions: [Int], mask: MLXArray?
    ) throws -> MLXArray {
        let heads = config.numAttentionHeads
        let kvHeads = config.numKeyValueHeads
        let headDim = config.headDim
        let length = positions.count

        let qkv = layer.qkv.matmul(x)
        let qDim = heads * headDim
        let kvDim = kvHeads * headDim
        let parts = qkv.split(indices: [qDim, qDim + kvDim], axis: -1)
        var q = parts[0].reshaped([1, length, heads, headDim]).transposed(0, 2, 1, 3)
        var k = parts[1].reshaped([1, length, kvHeads, headDim]).transposed(0, 2, 1, 3)
        var v = parts[2].reshaped([1, length, kvHeads, headDim]).transposed(0, 2, 1, 3)

        q = norm(q, layer.qNorm)
        k = norm(k, layer.kNorm)

        if let cache = caches[index] {
            k = concatenated([cache.keys, k], axis: 2)
            v = concatenated([cache.values, v], axis: 2)
        }
        if !layer.fullAttention, k.dim(2) > config.windowSize {
            let keep = k.dim(2) - config.windowSize
            k = k[0..., 0..., keep..., 0...]
            v = v[0..., 0..., keep..., 0...]
        }
        caches[index] = Cache(keys: k, values: v)

        // Cached keys keep their own positions; after a window trim they are the tail of the range
        // that ends at the query's position, which is the local coordinate system the model was
        // trained with.
        guard let queryEnd = positions.last else {
            throw PlamoError.invalidRequest(reason: "rope positions")
        }
        let keyCount = k.dim(2)
        let keyPositions = ((queryEnd - keyCount + 1)...queryEnd).map { $0 }
        let (qCos, qSin) = try layer.rotary.cosSin(positions: positions)
        let (kCos, kSin) = try layer.rotary.cosSin(positions: keyPositions)
        q = rotate(q, cos: qCos, sin: qSin)
        k = rotate(k, cos: kCos, sin: kSin)

        // One query token attends to every cached key, and a trimmed sliding cache holds at most
        // `windowSize` of them, so only a multi-token prefill needs a mask — and that one is built
        // once for the whole pass.
        var out = MLXFast.scaledDotProductAttention(
            queries: q, keys: k, values: v, scale: 1 / Float(headDim).squareRoot(), mask: mask)
        out = out.transposed(0, 2, 1, 3).reshaped([1, length, qDim])
        return layer.oProj.matmul(out)
    }

    /// PLaMo's Python rotates in float32 and casts the result back to the activation dtype. Keeping
    /// that cast (rather than rounding cos and sin first) is what keeps greedy output identical.
    private func rotate(_ x: MLXArray, cos: MLXArray, sin: MLXArray) -> MLXArray {
        let parts = x.split(parts: 2, axis: -1)
        let rotated = concatenated([-parts[1], parts[0]], axis: -1)
        return (x * cos + rotated * sin).asType(x.dtype)
    }

    private func mlp(_ x: MLXArray, layer: Layer) -> MLXArray {
        let gate = layer.gateUp.matmul(x).split(parts: 2, axis: -1)
        // mlx.nn.silu, without taking the whole MLXNN product for one activation.
        let activated = gate[0] * sigmoid(gate[0])
        return layer.downProj.matmul(activated * gate[1])
    }

    /// How many decode steps one graph carries. Reading each token back to the host costs about
    /// 0.2 ms, and chaining removes one such round trip per step, but a pending step keeps its
    /// activations and its KV copy alive: two steps per graph measured 0.0504 s per completion
    /// against 0.0531 s with a sync per token (60 interleaved requests each), while four and eight
    /// measured 0.0417 s and 0.0499 s because they no longer fit the 32 MB Metal buffer cache.
    public static let decodeChunk = 2

    /// Greedy decode, one token at a time, stopping on any of `stopIDs`. The token stays an array
    /// between steps; the host reads a chunk at once and truncates it at a stop token, so the
    /// output is the same as a sync per token.
    public func generate(prompt: [Int32], maxTokens: Int, stopIDs: Set<Int32>) throws -> [Int32] {
        resetCache()
        var logits = try self.logits(ids: prompt)
        var produced = [Int32]()
        produced.reserveCapacity(maxTokens)
        while produced.count < maxTokens {
            let chunk = min(Self.decodeChunk, maxTokens - produced.count)
            var tokens = [MLXArray]()
            tokens.reserveCapacity(chunk)
            for _ in 0..<chunk {
                let next = logits.argMax(axis: -1)
                tokens.append(next)
                logits = try self.logits(token: next)
            }
            if produced.isEmpty { Timing.mark("gen.prefill") }
            // `asArray` evaluates the whole chain, so this is the one host round trip per chunk.
            for value in stacked(tokens).asArray(Int32.self) {
                if stopIDs.contains(value) { return produced }
                produced.append(value)
            }
        }
        return produced
    }
}
