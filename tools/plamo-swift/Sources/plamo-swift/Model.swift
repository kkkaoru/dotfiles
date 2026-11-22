import Foundation
import MLX
import MLXNN

struct PlamoConfig: Decodable {
    struct RopeParameters: Decodable {
        let ropeType: String
        let ropeTheta: Float
        let factor: Float?
        let originalMaxPositionEmbeddings: Int?
        let betaFast: Float?
        let betaSlow: Float?

        enum CodingKeys: String, CodingKey {
            case ropeType = "rope_type"
            case ropeTheta = "rope_theta"
            case factor
            case originalMaxPositionEmbeddings = "original_max_position_embeddings"
            case betaFast = "beta_fast"
            case betaSlow = "beta_slow"
        }
    }

    let hiddenSize: Int
    let numHiddenLayers: Int
    let numAttentionHeads: Int
    let numKeyValueHeads: Int
    let headDim: Int
    let intermediateSize: Int
    let vocabSize: Int
    let rmsNormEps: Float
    let windowSize: Int
    let slidingWindowPattern: Int
    let ropeParameters: [String: RopeParameters]
    let eosTokenId: [Int32]
    let bosTokenId: Int32

    enum CodingKeys: String, CodingKey {
        case hiddenSize = "hidden_size"
        case numHiddenLayers = "num_hidden_layers"
        case numAttentionHeads = "num_attention_heads"
        case numKeyValueHeads = "num_key_value_heads"
        case headDim = "head_dim"
        case intermediateSize = "intermediate_size"
        case vocabSize = "vocab_size"
        case rmsNormEps = "rms_norm_eps"
        case windowSize = "window_size"
        case slidingWindowPattern = "sliding_window_pattern"
        case ropeParameters = "rope_parameters"
        case eosTokenId = "eos_token_id"
        case bosTokenId = "bos_token_id"
    }
}

/// One quantized matrix: 4-bit affine, group 64, exactly as the safetensors store it.
struct Quant {
    let weight: MLXArray
    let scales: MLXArray
    let biases: MLXArray?

    init(_ arrays: [String: MLXArray], _ prefix: String) throws {
        guard let weight = arrays["\(prefix).weight"], let scales = arrays["\(prefix).scales"] else {
            throw CocoaError(.keyValueValidation)
        }
        self.weight = weight
        self.scales = scales
        self.biases = arrays["\(prefix).biases"]
    }

    func matmul(_ x: MLXArray) -> MLXArray {
        MLX.quantizedMatmul(
            x, weight, scales: scales, biases: biases, transpose: true, groupSize: 64, bits: 4)
    }

    func embed(_ ids: MLXArray) -> MLXArray {
        MLX.dequantized(
            weight[ids], scales: scales[ids], biases: biases?[ids], groupSize: 64, bits: 4)
    }
}

/// Rotary embedding for one attention flavour: mlx_lm's plamo3 uses plain RoPE, with theta 1e6 on the
/// full-attention layers and 1e4 on the sliding ones. It ignores `rope_parameters`' yarn scaling, so
/// matching its output means ignoring it here too.
struct Rotary {
    let invFreq: MLXArray

    init(_ parameters: PlamoConfig.RopeParameters, headDim: Int) {
        let half = headDim / 2
        // MLX's RoPE uses base ** (arange(0, dim, 2) / dim), i.e. 2 * i / headDim.
        let exponent = MLX.arange(0, half, dtype: .float32) * 2 / Float(headDim)
        invFreq = 1 / pow(MLXArray(parameters.ropeTheta), exponent)
    }

    /// cos and sin for `positions`, shaped [1, 1, positions.count, headDim] to broadcast over heads.
    func cosSin(positions: [Int]) -> (MLXArray, MLXArray) {
        let t = MLXArray(positions.map { Float($0) })
        let freqs = t.expandedDimensions(axis: -1) * invFreq.expandedDimensions(axis: 0)
        let emb = concatenated([freqs, freqs], axis: -1)
        let shape = [1, 1, positions.count, invFreq.dim(0) * 2]
        return (cos(emb).reshaped(shape), sin(emb).reshaped(shape))
    }
}

final class PlamoModel {
    struct Layer {
        let preMixerNorm: MLXArray
        let postMixerNorm: MLXArray
        let preMLPNorm: MLXArray
        let postMLPNorm: MLXArray
        let qNorm: MLXArray
        let kNorm: MLXArray
        let qkv: Quant
        let oProj: Quant
        let gateUp: Quant
        let downProj: Quant
        let fullAttention: Bool
        let rotary: Rotary
    }

    struct Cache {
        var keys: MLXArray
        var values: MLXArray
    }

    let config: PlamoConfig
    private let layers: [Layer]
    private let embedTokens: Quant
    private let finalNorm: MLXArray
    private let lmHead: Quant
    private var caches: [Cache?]
    private var seen = 0

    static func load(directory: URL) throws -> PlamoModel {
        let config = try JSONDecoder().decode(
            PlamoConfig.self, from: try Data(contentsOf: directory.appendingPathComponent("config.json")))
        let arrays = try loadArrays(url: directory.appendingPathComponent("model.safetensors"))

        let full = Rotary(config.ropeParameters["full_attention"]!, headDim: config.headDim)
        let sliding = Rotary(config.ropeParameters["sliding_attention"]!, headDim: config.headDim)
        var layers = [Layer]()
        for index in 0..<config.numHiddenLayers {
            let base = "model.layers.layers.\(index)"
            let fullAttention = (index + 1) % config.slidingWindowPattern == 0
            layers.append(
                Layer(
                    preMixerNorm: arrays["\(base).pre_mixer_norm.weight"]!,
                    postMixerNorm: arrays["\(base).post_mixer_norm.weight"]!,
                    preMLPNorm: arrays["\(base).pre_mlp_norm.weight"]!,
                    postMLPNorm: arrays["\(base).post_mlp_norm.weight"]!,
                    qNorm: arrays["\(base).mixer.q_norm.weight"]!,
                    kNorm: arrays["\(base).mixer.k_norm.weight"]!,
                    qkv: try Quant(arrays, "\(base).mixer.qkv_proj"),
                    oProj: try Quant(arrays, "\(base).mixer.o_proj"),
                    gateUp: try Quant(arrays, "\(base).mlp.gate_up_proj"),
                    downProj: try Quant(arrays, "\(base).mlp.down_proj"),
                    fullAttention: fullAttention,
                    rotary: fullAttention ? full : sliding))
        }
        let model = PlamoModel(
            config: config,
            layers: layers,
            embedTokens: try Quant(arrays, "model.embed_tokens"),
            finalNorm: arrays["model.norm.weight"]!,
            lmHead: try Quant(arrays, "lm_head"))
        model.caches = Array(repeating: nil, count: config.numHiddenLayers)
        return model
    }

    private init(
        config: PlamoConfig, layers: [Layer], embedTokens: Quant, finalNorm: MLXArray, lmHead: Quant
    ) {
        self.config = config
        self.layers = layers
        self.embedTokens = embedTokens
        self.finalNorm = finalNorm
        self.lmHead = lmHead
        self.caches = []
    }

    func resetCache() {
        caches = Array(repeating: nil, count: config.numHiddenLayers)
        seen = 0
    }

    /// The offsets PLaMo's norms use; all four differ, which the model card calls out.
    /// The scale stays in the activation's dtype: a float32 weight would upcast every layer.
    private func norm(_ x: MLXArray, _ weight: MLXArray, offset: Float) -> MLXArray {
        MLXFast.rmsNorm(x, weight: (weight + offset).asType(x.dtype), eps: config.rmsNormEps)
    }

    /// logits of the last position, and the KV cache grows by `ids.count` tokens.
    func logits(ids: [Int32], onStage: ((String, MLXArray) -> Void)? = nil) -> MLXArray {
        let length = ids.count
        var hidden = embedTokens.embed(MLXArray(ids)).reshaped([1, length, config.hiddenSize])
        onStage?("embed", hidden)
        let start = seen
        let queryPositions = (start..<(start + length)).map { $0 }

        for (index, layer) in layers.enumerated() {
            let residual = hidden
            var h = norm(hidden, layer.preMixerNorm, offset: 1)
            onStage?("L\(index).pre_mixer", h)
            h = attention(h, layer: layer, index: index, positions: queryPositions)
            onStage?("L\(index).mixer", h)
            hidden = residual + norm(h, layer.postMixerNorm, offset: 1.0 / 5)

            let residualMLP = hidden
            h = norm(hidden, layer.preMLPNorm, offset: 1)
            h = mlp(h, layer: layer)
            onStage?("L\(index).mlp", h)
            hidden = residualMLP + norm(h, layer.postMLPNorm, offset: 1 / pow(Float(5), 1.5))
        }
        seen += length

        hidden = norm(hidden, finalNorm, offset: 1)
        let last = hidden[0..., (length - 1)..<length, 0...]
        return lmHead.matmul(last)
    }

    private func attention(_ x: MLXArray, layer: Layer, index: Int, positions: [Int]) -> MLXArray {
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

        q = norm(q, layer.qNorm, offset: 1)
        k = norm(k, layer.kNorm, offset: 1)

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
        let keyCount = k.dim(2)
        let queryEnd = positions.last!
        let keyPositions = ((queryEnd - keyCount + 1)...queryEnd).map { $0 }
        let (qCos, qSin) = layer.rotary.cosSin(positions: positions)
        let (kCos, kSin) = layer.rotary.cosSin(positions: keyPositions)
        q = rotate(q, cos: qCos, sin: qSin)
        k = rotate(k, cos: kCos, sin: kSin)

        // One query token attends to every cached key, and a trimmed sliding cache holds at most
        // `windowSize` of them, so only a multi-token prefill needs a mask.
        var mask: MLXArray? = nil
        if length > 1 {
            // The mask has to carry the activation dtype: the model runs in bfloat16 and MLX will not
            // promote a float32 mask into it.
            mask = attentionMask(
                queries: length, keys: keyCount, window: layer.fullAttention ? nil : config.windowSize
            ).asType(q.dtype)
        }
        var out = MLXFast.scaledDotProductAttention(
            queries: q, keys: k, values: v, scale: 1 / Float(headDim).squareRoot(), mask: mask)
        out = out.transposed(0, 2, 1, 3).reshaped([1, length, qDim])
        return layer.oProj.matmul(out)
    }

    /// cos and sin are float32 for accuracy; MLX's own RoPE casts them to the input dtype, and so
    /// does this, or the multiplication would upcast q and k and run attention in float32.
    private func rotate(_ x: MLXArray, cos: MLXArray, sin: MLXArray) -> MLXArray {
        let parts = x.split(parts: 2, axis: -1)
        let rotated = concatenated([-parts[1], parts[0]], axis: -1)
        return x * cos.asType(x.dtype) + rotated * sin.asType(x.dtype)
    }

    private func attentionMask(queries: Int, keys: Int, window: Int?) -> MLXArray {
        var values = [Float](repeating: -Float.infinity, count: queries * keys)
        let offset = keys - queries
        for i in 0..<queries {
            let limit = i + offset
            let floor = window.map { max(0, limit - $0 + 1) } ?? 0
            for j in floor...limit { values[i * keys + j] = 0 }
        }
        return MLXArray(values).reshaped([1, 1, queries, keys])
    }

    private func mlp(_ x: MLXArray, layer: Layer) -> MLXArray {
        let gate = layer.gateUp.matmul(x).split(parts: 2, axis: -1)
        return layer.downProj.matmul(silu(gate[0]) * gate[1])
    }

    /// Greedy decode, one token at a time, stopping on any of `stopIDs`.
    func generate(prompt: [Int32], maxTokens: Int, stopIDs: Set<Int32>) -> [Int32] {
        resetCache()
        var logits = self.logits(ids: prompt)
        var produced = [Int32]()
        for _ in 0..<maxTokens {
            let next = logits.argMax(axis: -1).item(Int32.self)
            if stopIDs.contains(next) { break }
            produced.append(next)
            logits = self.logits(ids: [next])
        }
        return produced
    }
}
