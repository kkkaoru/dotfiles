import Foundation

/// One weight the graph binds: the name the graph uses, the checkpoint's own name and the shape.
public struct WeightInput {
    /// The name inside the MIL text. A dot is not allowed: coremlc writes an input name into the
    /// MIL text unquoted, and MLModel then fails to parse the model.
    public let input: String
    /// The checkpoint's name for this weight.
    public let checkpoint: String
    /// The weight's shape, as the graph binds it.
    public let shape: [Int]
}

/// The RoPE tables and the negation constant, as graph inputs rather than MIL immediates.
///
/// An immediate is a FLOAT16 blob inside the compiled model, and BNNS then faults writing through
/// it. Swift already computes the tables, so it can hold these too.
public struct RoPETables {
    /// The cosine table, `[1, 1, 1, headDim]`.
    public let cos: String
    /// The sine table, `[1, 1, 1, headDim]`.
    public let sin: String
    /// A single `-1`, for the half-pair swap.
    public let negOne: String
}

/// The two attention inputs that are not weights: the query scale and the cache-row mask.
public struct AttentionInputs {
    /// The query scale, `[1]`.
    public let scale: String
    /// The cache-row mask, `[1, 1, cacheLength]`.
    public let mask: String
}

/// The key and value caches this step reads and writes, as graph input and output.
public struct LayerCache {
    /// The key cache, `[1, kvHeads, cacheLength, headDim]`.
    public let keys: String
    /// The value cache, `[1, kvHeads, cacheLength, headDim]`.
    public let values: String
}

/// One decoder layer of PLaMo 3 as an ML Program, for a single-token decode step.
///
/// This mirrors `PlamoModel.forward` exactly — pre-mixer norm, attention with RoPE over the new
/// token only, the cache concatenation, the sliding window's trim, the residual's own post norm,
/// then the same shape for the MLP — so a token produced here is the token MLX produces.
///
/// The cache is a graph input and output rather than MIL state: Swift already owns it, and a
/// functional step keeps the graph free of Core ML state's lifetime rules. A sliding layer's cache
/// holds exactly `windowSize` keys, so the trim drops the oldest row before the new one is
/// appended — the same keys MLX keeps.
public struct DecoderLayerGraph {
    /// The names the graph uses for one layer's tensors.
    public struct InputNames: Sendable {
        public let qkv = "w_qkv"
        public let output = "w_output"
        public let gateUp = "w_gate_up"
        public let down = "w_down"
        public let preMixerNorm = "w_pre_mixer_norm"
        public let postMixerNorm = "w_post_mixer_norm"
        public let preMLPNorm = "w_pre_mlp_norm"
        public let postMLPNorm = "w_post_mlp_norm"
        public let qNorm = "w_q_norm"
        public let kNorm = "w_k_norm"

        /// The fixed graph-side names above.
        public init() {}
    }

    /// The names and shapes of one layer's tensors, as the checkpoint spells them.
    public struct Weights {
        /// The fused qkv projection, `[hidden, q + 2 * kv]`.
        public let qkv: String
        /// The attention output projection, `[q, hidden]`.
        public let output: String
        /// The fused SwiGLU gate and up projection, `[hidden, 2 * intermediate]`.
        public let gateUp: String
        /// The MLP down projection, `[intermediate, hidden]`.
        public let down: String
        /// The norm before the attention block.
        public let preMixerNorm: String
        /// The norm after the attention residual.
        public let postMixerNorm: String
        /// The norm before the MLP.
        public let preMLPNorm: String
        /// The norm after the MLP residual.
        public let postMLPNorm: String
        /// The per-head norm on the queries.
        public let qNorm: String
        /// The per-head norm on the keys.
        public let kNorm: String

        /// The names the graph uses for those tensors.
        public let input: InputNames

        /// The checkpoint's names for one layer, under `prefix`.
        public init(prefix: String, layer: Int) {
            input = InputNames()
            let base = "\(prefix).layers.layers.\(layer)"
            qkv = "\(base).mixer.qkv_proj.weight"
            output = "\(base).mixer.o_proj.weight"
            gateUp = "\(base).mlp.gate_up_proj.weight"
            down = "\(base).mlp.down_proj.weight"
            preMixerNorm = "\(base).pre_mixer_norm.weight"
            postMixerNorm = "\(base).post_mixer_norm.weight"
            preMLPNorm = "\(base).pre_mlp_norm.weight"
            postMLPNorm = "\(base).post_mlp_norm.weight"
            qNorm = "\(base).mixer.q_norm.weight"
            kNorm = "\(base).mixer.k_norm.weight"
        }
    }

    /// The model configuration this layer reads its shapes from.
    public let config: PlamoConfig
    /// The layer's weights, as the checkpoint spells them.
    public let weights: Weights
    /// Keys the cache holds before this step. A sliding layer holds `windowSize`; a full layer
    /// holds the whole context.
    public let cacheLength: Int

    /// One layer's graph for the given configuration, weights and cache length.
    public init(config: PlamoConfig, weights: Weights, cacheLength: Int) {
        self.config = config
        self.weights = weights
        self.cacheLength = cacheLength
    }

    private var heads: Int { config.numAttentionHeads }
    private var kvHeads: Int { config.numKeyValueHeads }
    private var headDim: Int { config.headDim }
    private var hidden: Int { config.hiddenSize }
    private var qDim: Int { heads * headDim }
    private var kvDim: Int { kvHeads * headDim }
    private var half: Int { headDim / 2 }

    /// The graph: `x` in, `y` out, with the caches and every weight as inputs.
    public func render() -> String { buildProgram().render() }

    /// The graph's weight inputs, in the order the checkpoint names them.
    internal var weightInputs: [WeightInput] {
        [
            WeightInput(
                input: weights.input.qkv, checkpoint: weights.qkv,
                shape: [qDim + 2 * kvDim, hidden]),
            WeightInput(
                input: weights.input.output, checkpoint: weights.output, shape: [hidden, qDim]),
            WeightInput(
                input: weights.input.gateUp, checkpoint: weights.gateUp,
                shape: [2 * config.intermediateSize, hidden]),
            WeightInput(
                input: weights.input.down, checkpoint: weights.down,
                shape: [hidden, config.intermediateSize]),
            WeightInput(
                input: weights.input.preMixerNorm, checkpoint: weights.preMixerNorm,
                shape: [hidden]),
            WeightInput(
                input: weights.input.postMixerNorm, checkpoint: weights.postMixerNorm,
                shape: [hidden]),
            WeightInput(
                input: weights.input.preMLPNorm, checkpoint: weights.preMLPNorm, shape: [hidden]),
            WeightInput(
                input: weights.input.postMLPNorm, checkpoint: weights.postMLPNorm,
                shape: [hidden]),
            WeightInput(input: weights.input.qNorm, checkpoint: weights.qNorm, shape: [headDim]),
            WeightInput(input: weights.input.kNorm, checkpoint: weights.kNorm, shape: [headDim]),
        ]
    }

    /// The same graph, optionally cut short so a failing operation can be found by bisection.
    internal func buildProgram(truncateTo ops: Int? = nil) -> MILText {
        var program = MILText()
        let x = MILText.Tensor("x", [1, 1, hidden])
        let rope = RoPETables(cos: "cos", sin: "sin", negOne: "neg_one")
        let scale = "attn_scale"
        let mask = "mask"
        let scalars = AttentionInputs(scale: scale, mask: mask)
        let cache = LayerCache(keys: "k_cache", values: "v_cache")
        for weight in weightInputs {
            program.addInput(MILText.Tensor(weight.input, weight.shape))
            program.annotate("\(weight.input) = \(weight.checkpoint)")
        }
        for tensor in [
            x, MILText.Tensor(rope.cos, [1, 1, 1, headDim]),
            MILText.Tensor(rope.sin, [1, 1, 1, headDim]), MILText.Tensor(rope.negOne, [1]),
            MILText.Tensor(scale, [1]),
            // Zero for a cache row that holds a real token, a large negative value for a row the
            // window has not reached yet: without it the first tokens attend over empty rows.
            MILText.Tensor(mask, [1, 1, cacheLength]),
            MILText.Tensor(cache.keys, [1, kvHeads, cacheLength, headDim]),
            MILText.Tensor(cache.values, [1, kvHeads, cacheLength, headDim]),
        ] {
            program.addInput(tensor)
        }

        let x2 = MILText.Tensor("x2", [1, hidden])
        program.reshape(x.name, into: x2, shape: [1, hidden])

        let pre = normalized(
            program: &program, name: "pre_mixer", x: x2, weight: weights.input.preMixerNorm,
            into: MILText.Tensor("pre_mixer_out", [1, hidden]))
        let attended = attention(
            program: &program, x: pre, rope: rope, scalars: scalars, cache: cache)

        let residual = MILText.Tensor("residual", [1, hidden])
        program.add(x2.name, attended.mixed.name, into: residual)
        let postMixer = normalized(
            program: &program, name: "post_mixer", x: residual, weight: weights.input.postMixerNorm,
            into: MILText.Tensor("post_mixer_out", [1, hidden]))
        let hiddenAfterMixer = MILText.Tensor("hidden_after_mixer", [1, hidden])
        program.add(residual.name, postMixer.name, into: hiddenAfterMixer)

        let preMLP = normalized(
            program: &program, name: "pre_mlp", x: hiddenAfterMixer,
            weight: weights.input.preMLPNorm, into: MILText.Tensor("pre_mlp_out", [1, hidden]))
        let mlp = mlp(program: &program, x: preMLP)
        let postMLP = normalized(
            program: &program, name: "post_mlp", x: mlp, weight: weights.input.postMLPNorm,
            into: MILText.Tensor("post_mlp_out", [1, hidden]))
        let summed = MILText.Tensor("summed", [1, hidden])
        program.add(hiddenAfterMixer.name, postMLP.name, into: summed)
        let y = MILText.Tensor("y", [1, 1, hidden])
        program.reshape(summed.name, into: y, shape: [1, 1, hidden])
        program.setOutputs([y, attended.keys, attended.values])
        if let ops { program.truncate(ops: ops) }
        return program
    }

    /// What the attention block produces: the mixer's output and the two caches to keep.
    private struct Attended {
        let mixed: MILText.Tensor
        let keys: MILText.Tensor
        let values: MILText.Tensor
    }
    /// `x / rms(x) * w`, over the last axis.
    private func normalized(
        program: inout MILText, name: String, x: MILText.Tensor, weight: String,
        into y: MILText.Tensor
    ) -> MILText.Tensor {
        let squares = MILText.Tensor("\(name)_squares", x.shape)
        program.multiply(x.name, x.name, into: squares)
        // `keepDims` keeps every axis, so the reduced shape is the input with the last axis at 1.
        // Declaring it as [1, 1] instead makes the plan's buffer layout disagree with the op's
        // real output, and BNNS then writes outside its own buffer.
        let reducedShape = Array(x.shape.dropLast()) + [1]
        let mean = MILText.Tensor("\(name)_mean", reducedShape)
        program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
        let inverse = MILText.Tensor("\(name)_inverse", reducedShape)
        program.rsqrt(mean.name, into: inverse, epsilon: Double(config.rmsNormEps))
        let scaled = MILText.Tensor("\(name)_scaled", x.shape)
        program.multiply(x.name, inverse.name, into: scaled)
        let shaped = MILText.Tensor("\(name)_weight", [1, x.shape.last ?? 1])
        program.reshape(weight, into: shaped, shape: [1, x.shape.last ?? 1])
        program.multiply(scaled.name, shaped.name, into: y)
        return y
    }

    private func attention(
        program: inout MILText, x: MILText.Tensor, rope: RoPETables, scalars: AttentionInputs,
        cache: LayerCache
    ) -> Attended {
        let qkv = MILText.Tensor("qkv", [1, qDim + 2 * kvDim])
        program.matmul(x.name, weights.input.qkv, into: qkv, transposeY: true)

        let qFlat = MILText.Tensor("q_flat", [1, 1, qDim])
        program.sliceLast(qkv.name, into: qFlat, begin: [0, 0], size: [1, qDim])
        let kFlat = MILText.Tensor("k_flat", [1, 1, kvDim])
        program.sliceLast(qkv.name, into: kFlat, begin: [0, qDim], size: [1, kvDim])
        let vFlat = MILText.Tensor("v_flat", [1, 1, kvDim])
        program.sliceLast(qkv.name, into: vFlat, begin: [0, qDim + kvDim], size: [1, kvDim])

        let q = heads3(program: &program, flat: qFlat, name: "q", heads: heads)
        let k = heads3(program: &program, flat: kFlat, name: "k", heads: kvHeads)
        let v = heads3(program: &program, flat: vFlat, name: "v", heads: kvHeads)

        let qNormed = normalized(
            program: &program, name: "q_norm", x: q, weight: weights.input.qNorm,
            into: MILText.Tensor("q_norm_out", q.shape))
        let kNormed = normalized(
            program: &program, name: "k_norm", x: k, weight: weights.input.kNorm,
            into: MILText.Tensor("k_norm_out", k.shape))

        let qRotated = rotate(program: &program, x: qNormed, name: "q_rot", rope: rope)
        let kRotated = rotate(program: &program, x: kNormed, name: "k_rot", rope: rope)

        let kept = appendToCache(program: &program, k: kRotated, v: v, cache: cache)
        let mixed = attend(program: &program, q: qRotated, cache: kept, scalars: scalars)
        return Attended(
            mixed: mixed,
            keys: MILText.Tensor(kept.keys, [1, kvHeads, cacheLength, headDim]),
            values: MILText.Tensor(kept.values, [1, kvHeads, cacheLength, headDim]))
    }

    /// The window drops its oldest row before the new one is appended, not after: slicing the
    /// result of a concat crashes the plan builder (SIGBUS), while slicing an input is fine.
    private func appendToCache(
        program: inout MILText, k: MILText.Tensor, v: MILText.Tensor, cache: LayerCache
    ) -> LayerCache {
        let keptKeys = MILText.Tensor("keys_kept", [1, kvHeads, cacheLength - 1, headDim])
        program.sliceLast(
            cache.keys, into: keptKeys, begin: [0, 0, 1, 0],
            size: [1, kvHeads, cacheLength - 1, headDim])
        let keys = MILText.Tensor("keys_trimmed", [1, kvHeads, cacheLength, headDim])
        program.concat([keptKeys.name, k.name], into: keys, axis: 2)
        let keptValues = MILText.Tensor("values_kept", [1, kvHeads, cacheLength - 1, headDim])
        program.sliceLast(
            cache.values, into: keptValues, begin: [0, 0, 1, 0],
            size: [1, kvHeads, cacheLength - 1, headDim])
        let values = MILText.Tensor("values_trimmed", [1, kvHeads, cacheLength, headDim])
        program.concat([keptValues.name, v.name], into: values, axis: 2)
        return LayerCache(keys: keys.name, values: values.name)
    }

    /// Attention written out: q @ kᵀ, scaled, softmax, then @ v. The dedicated op is newer than the
    /// opset coremlc ships, and these four are accepted by every opset. The query has more heads
    /// than the cache, so the group is a dimension of its own: the batched matmul shares one k/v
    /// head across its group, which is what GQA means. The batch is rank 3, not 4: a rank-4 matmul
    /// fails the plan builder with error -14.
    private func attend(
        program: inout MILText, q: MILText.Tensor, cache: LayerCache, scalars: AttentionInputs
    ) -> MILText.Tensor {
        let groupSize = heads / kvHeads
        let keysT = MILText.Tensor("keys_t", [1, kvHeads, headDim, cacheLength])
        program.transpose(cache.keys, into: keysT, perm: [0, 1, 3, 2])
        let keysG = MILText.Tensor("keys_g", [kvHeads, headDim, cacheLength])
        program.reshape(keysT.name, into: keysG, shape: keysG.shape)
        let qG = MILText.Tensor("q_g", [kvHeads, groupSize, headDim])
        program.reshape(q.name, into: qG, shape: qG.shape)
        let scores = MILText.Tensor("scores", [kvHeads, groupSize, cacheLength])
        program.matmul(qG.name, keysG.name, into: scores)
        let scaledScores = MILText.Tensor("scores_scaled", scores.shape)
        program.multiply(scores.name, scalars.scale, into: scaledScores)
        let maskedScores = MILText.Tensor("scores_masked", scores.shape)
        program.add(scaledScores.name, scalars.mask, into: maskedScores)
        let probs = MILText.Tensor("probs", scores.shape)
        program.softmax(maskedScores.name, into: probs, axis: -1)
        let valuesG = MILText.Tensor("values_g", [kvHeads, cacheLength, headDim])
        program.reshape(cache.values, into: valuesG, shape: valuesG.shape)
        let attendedG = MILText.Tensor("attended_g", [kvHeads, groupSize, headDim])
        program.matmul(probs.name, valuesG.name, into: attendedG)
        let attended = MILText.Tensor("attended", [1, heads, 1, headDim])
        program.reshape(attendedG.name, into: attended, shape: attended.shape)

        let merged = MILText.Tensor("merged", [1, 1, qDim])
        let transposed = MILText.Tensor("attended_t", [1, 1, heads, headDim])
        program.transpose(attended.name, into: transposed, perm: [0, 2, 1, 3])
        program.reshape(transposed.name, into: merged, shape: [1, 1, qDim])

        let mixed = MILText.Tensor("mixed", [1, hidden])
        let merged2 = MILText.Tensor("merged2", [1, qDim])
        program.reshape(merged.name, into: merged2, shape: [1, qDim])
        program.matmul(merged2.name, weights.input.output, into: mixed, transposeY: true)
        return mixed
    }

    /// `[1, 1, dim]` to `[1, heads, 1, headDim]`.
    private func heads3(
        program: inout MILText, flat: MILText.Tensor, name: String, heads: Int
    ) -> MILText.Tensor {
        let shaped = MILText.Tensor("\(name)_shaped", [1, 1, heads, headDim])
        program.reshape(flat.name, into: shaped, shape: [1, 1, heads, headDim])
        let moved = MILText.Tensor("\(name)_heads", [1, heads, 1, headDim])
        program.transpose(shaped.name, into: moved, perm: [0, 2, 1, 3])
        return moved
    }

    /// PLaMo rotates half-pairs: `x * cos + [-b, a] * sin`.
    private func rotate(
        program: inout MILText, x: MILText.Tensor, name: String, rope: RoPETables
    ) -> MILText.Tensor {
        let shape = x.shape
        let first = MILText.Tensor("\(name)_first", [1, shape[1], 1, half])
        program.sliceLast(x.name, into: first, begin: [0, 0, 0, 0], size: [1, shape[1], 1, half])
        let second = MILText.Tensor("\(name)_second", [1, shape[1], 1, half])
        program.sliceLast(
            x.name, into: second, begin: [0, 0, 0, half], size: [1, shape[1], 1, half])
        let negated = MILText.Tensor("\(name)_negated", [1, shape[1], 1, half])
        program.multiply(second.name, rope.negOne, into: negated)
        let rotated = MILText.Tensor("\(name)_swapped", [1, shape[1], 1, headDim])
        program.concat([negated.name, first.name], into: rotated, axis: 3)
        let scaled = MILText.Tensor("\(name)_scaled", shape)
        program.multiply(x.name, rope.cos, into: scaled)
        let turned = MILText.Tensor("\(name)_turned", shape)
        program.multiply(rotated.name, rope.sin, into: turned)
        let out = MILText.Tensor("\(name)_out", shape)
        program.add(scaled.name, turned.name, into: out)
        return out
    }

    private func mlp(program: inout MILText, x: MILText.Tensor) -> MILText.Tensor {
        let intermediate = config.intermediateSize
        let gateUp = MILText.Tensor("gate_up", [1, 2 * intermediate])
        program.matmul(x.name, weights.input.gateUp, into: gateUp, transposeY: true)
        let gate = MILText.Tensor("gate", [1, intermediate])
        program.sliceLast(gateUp.name, into: gate, begin: [0, 0], size: [1, intermediate])
        let up = MILText.Tensor("up", [1, intermediate])
        program.sliceLast(
            gateUp.name, into: up, begin: [0, intermediate], size: [1, intermediate])
        let activated = MILText.Tensor("activated", [1, intermediate])
        program.sigmoid(gate.name, into: activated)
        let gated = MILText.Tensor("gated", [1, intermediate])
        program.multiply(gate.name, activated.name, into: gated)
        let product = MILText.Tensor("product", [1, intermediate])
        program.multiply(gated.name, up.name, into: product)
        let out = MILText.Tensor("mlp_out", [1, hidden])
        program.matmul(product.name, weights.input.down, into: out, transposeY: true)
        return out
    }
}
