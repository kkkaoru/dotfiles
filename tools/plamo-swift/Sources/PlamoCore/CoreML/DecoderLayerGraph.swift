import Foundation

/// One decoder layer of PLaMo 3 as an ML Program, for a single-token decode step.
///
/// This mirrors `PlamoModel.forward` exactly — pre-mixer norm, attention with RoPE over the new
/// token only, the cache concatenation, the sliding window's trim, the residual's own post norm,
/// then the same shape for the MLP — so a token produced here is the token MLX produces.
///
/// The cache is a graph input and output rather than MIL state: Swift already owns it, and a
/// functional step keeps the graph free of Core ML state's lifetime rules. A sliding layer's cache
/// holds exactly `windowSize` keys, so the trim is a `slice_by_size` of the last window after the
/// concatenation — the same keys MLX keeps.
public struct DecoderLayerGraph {
    /// The names and shapes of one layer's tensors, as the checkpoint spells them.
    public struct Weights {
        public let qkv: String
        public let output: String
        public let gateUp: String
        public let down: String
        public let preMixerNorm: String
        public let postMixerNorm: String
        public let preMLPNorm: String
        public let postMLPNorm: String
        public let qNorm: String
        public let kNorm: String

        public init(
            prefix: String, layer: Int
        ) {
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

    public let config: PlamoConfig
    public let weights: Weights
    /// Keys the cache holds before this step. A sliding layer holds `windowSize`; a full layer
    /// holds the whole context.
    public let cacheLength: Int

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
    public func render() -> String {
        var program = MILText()
        let x = MILText.Tensor("x", [1, 1, hidden])
        let cos = MILText.Tensor("cos", [1, 1, 1, half])
        let sin = MILText.Tensor("sin", [1, 1, 1, half])
        let kCache = MILText.Tensor("k_cache", [1, kvHeads, cacheLength, headDim])
        let vCache = MILText.Tensor("v_cache", [1, kvHeads, cacheLength, headDim])
        let weightTensors: [(String, [Int])] = [
            (weights.qkv, [hidden, qDim + 2 * kvDim]),
            (weights.output, [qDim, hidden]),
            (weights.gateUp, [hidden, 2 * config.intermediateSize]),
            (weights.down, [config.intermediateSize, hidden]),
            (weights.preMixerNorm, [hidden]),
            (weights.postMixerNorm, [hidden]),
            (weights.preMLPNorm, [hidden]),
            (weights.postMLPNorm, [hidden]),
            (weights.qNorm, [headDim]),
            (weights.kNorm, [headDim]),
        ]
        for (name, shape) in weightTensors {
            program.addInput(MILText.Tensor(name, shape))
        }
        for tensor in [x, cos, sin, kCache, vCache] {
            program.addInput(tensor)
        }

        let x2 = MILText.Tensor("x2", [1, hidden])
        program.reshape(x.name, into: x2, shape: [1, hidden])

        let pre = normalized(program: &program, name: "pre_mixer", x: x2, weight: weights.preMixerNorm, into: MILText.Tensor("pre_mixer_out", [1, hidden]))
        let (mixer, keys, values) = attention(
            program: &program, x: pre, cos: cos.name, sin: sin.name, kCache: kCache.name,
            vCache: vCache.name)

        let residual = MILText.Tensor("residual", [1, hidden])
        program.add(x2.name, mixer.name, into: residual)
        let postMixer = normalized(program: &program, name: "post_mixer", x: residual, weight: weights.postMixerNorm, into: MILText.Tensor("post_mixer_out", [1, hidden]))
        let hiddenAfterMixer = MILText.Tensor("hidden_after_mixer", [1, hidden])
        program.add(residual.name, postMixer.name, into: hiddenAfterMixer)

        let preMLP = normalized(program: &program, name: "pre_mlp", x: hiddenAfterMixer, weight: weights.preMLPNorm, into: MILText.Tensor("pre_mlp_out", [1, hidden]))
        let mlp = mlp(program: &program, x: preMLP)
        let postMLP = normalized(program: &program, name: "post_mlp", x: mlp, weight: weights.postMLPNorm, into: MILText.Tensor("post_mlp_out", [1, hidden]))
        let summed = MILText.Tensor("summed", [1, hidden])
        program.add(hiddenAfterMixer.name, postMLP.name, into: summed)
        let y = MILText.Tensor("y", [1, 1, hidden])
        program.reshape(summed.name, into: y, shape: [1, 1, hidden])
        program.setOutputs([y, keys, values])
        return program.render()
    }

    /// `x / rms(x) * w`, over the last axis.
    private func normalized(
        program: inout MILText, name: String, x: MILText.Tensor, weight: String, into y: MILText.Tensor
    ) -> MILText.Tensor {
        let squares = MILText.Tensor("\(name)_squares", x.shape)
        program.multiply(x.name, x.name, into: squares)
        let mean = MILText.Tensor("\(name)_mean", [1, 1])
        program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
        let shifted = MILText.Tensor("\(name)_shifted", [1, 1])
        program.addScalar(mean.name, into: shifted, epsilon: Double(config.rmsNormEps))
        let inverse = MILText.Tensor("\(name)_inverse", [1, 1])
        program.rsqrt(shifted.name, into: inverse)
        let scaled = MILText.Tensor("\(name)_scaled", x.shape)
        program.multiply(x.name, inverse.name, into: scaled)
        let shaped = MILText.Tensor("\(name)_weight", [1, x.shape.last ?? 1])
        program.reshape(weight, into: shaped, shape: [1, x.shape.last ?? 1])
        program.multiply(scaled.name, shaped.name, into: y)
        return y
    }

    private func attention(
        program: inout MILText, x: MILText.Tensor, cos: String, sin: String, kCache: String,
        vCache: String
    ) -> (mixed: MILText.Tensor, keys: MILText.Tensor, values: MILText.Tensor) {
        let qkv = MILText.Tensor("qkv", [1, qDim + 2 * kvDim])
        program.matmul(x.name, weights.qkv, into: qkv)

        let qFlat = MILText.Tensor("q_flat", [1, 1, qDim])
        program.sliceLast(qkv.name, into: qFlat, begin: [0, 0], size: [1, qDim])
        let kFlat = MILText.Tensor("k_flat", [1, 1, kvDim])
        program.sliceLast(qkv.name, into: kFlat, begin: [0, qDim], size: [1, kvDim])
        let vFlat = MILText.Tensor("v_flat", [1, 1, kvDim])
        program.sliceLast(qkv.name, into: vFlat, begin: [0, qDim + kvDim], size: [1, kvDim])

        let q = heads3(program: &program, flat: qFlat, name: "q", heads: heads)
        let k = heads3(program: &program, flat: kFlat, name: "k", heads: kvHeads)
        let v = heads3(program: &program, flat: vFlat, name: "v", heads: kvHeads)

        let qNormed = normalized(program: &program, name: "q_norm", x: q, weight: weights.qNorm, into: MILText.Tensor("q_norm_out", q.shape))
        let kNormed = normalized(program: &program, name: "k_norm", x: k, weight: weights.kNorm, into: MILText.Tensor("k_norm_out", k.shape))

        let qRotated = rotate(program: &program, x: qNormed, name: "q_rot", cos: cos, sin: sin)
        let kRotated = rotate(program: &program, x: kNormed, name: "k_rot", cos: cos, sin: sin)

        let keys = MILText.Tensor("keys", [1, kvHeads, cacheLength + 1, headDim])
        program.concat([kCache, kRotated.name], into: keys, axis: 2)
        let values = MILText.Tensor("values", [1, kvHeads, cacheLength + 1, headDim])
        program.concat([vCache, v.name], into: values, axis: 2)

        // A sliding layer keeps exactly its window; a full layer keeps its whole context.
        let trimmed = MILText.Tensor("keys_trimmed", [1, kvHeads, cacheLength, headDim])
        program.sliceLast(keys.name, into: trimmed, begin: [0, 0, 1, 0], size: [1, kvHeads, cacheLength, headDim])
        let trimmedValues = MILText.Tensor("values_trimmed", [1, kvHeads, cacheLength, headDim])
        program.sliceLast(values.name, into: trimmedValues, begin: [0, 0, 1, 0], size: [1, kvHeads, cacheLength, headDim])

        // Attention written out: q @ kᵀ, scaled, softmax, then @ v. The dedicated op is newer
        // than the opset coremlc ships, and these four are accepted by every opset.
        let keysT = MILText.Tensor("keys_t", [1, kvHeads, headDim, cacheLength])
        program.transpose(trimmed.name, into: keysT, perm: [0, 1, 3, 2])
        let scores = MILText.Tensor("scores", [1, heads, 1, cacheLength])
        program.matmul(qRotated.name, keysT.name, into: scores)
        let scaledScores = MILText.Tensor("scores_scaled", [1, heads, 1, cacheLength])
        program.multiplyScalar(scores.name, into: scaledScores, scale: 1 / Double(headDim).squareRoot())
        let probs = MILText.Tensor("probs", [1, heads, 1, cacheLength])
        program.softmax(scaledScores.name, into: probs, axis: -1)
        let attended = MILText.Tensor("attended", [1, heads, 1, headDim])
        program.matmul(probs.name, trimmedValues.name, into: attended)

        let merged = MILText.Tensor("merged", [1, 1, qDim])
        let transposed = MILText.Tensor("attended_t", [1, 1, heads, headDim])
        program.transpose(attended.name, into: transposed, perm: [0, 2, 1, 3])
        program.reshape(transposed.name, into: merged, shape: [1, 1, qDim])

        let mixed = MILText.Tensor("mixed", [1, hidden])
        let merged2 = MILText.Tensor("merged2", [1, qDim])
        program.reshape(merged.name, into: merged2, shape: [1, qDim])
        program.matmul(merged2.name, weights.output, into: mixed)
        return (mixed, trimmed, trimmedValues)
    }

    /// `[1, 1, dim]` to `[1, heads, 1, headDim]`.
    private func heads3(program: inout MILText, flat: MILText.Tensor, name: String, heads: Int) -> MILText.Tensor {
        let shaped = MILText.Tensor("\(name)_shaped", [1, 1, heads, headDim])
        program.reshape(flat.name, into: shaped, shape: [1, 1, heads, headDim])
        let moved = MILText.Tensor("\(name)_heads", [1, heads, 1, headDim])
        program.transpose(shaped.name, into: moved, perm: [0, 2, 1, 3])
        return moved
    }

    /// PLaMo rotates half-pairs: `x * cos + [-b, a] * sin`.
    private func rotate(program: inout MILText, x: MILText.Tensor, name: String, cos: String, sin: String) -> MILText.Tensor {
        let shape = x.shape
        let first = MILText.Tensor("\(name)_first", [1, shape[1], 1, half])
        program.sliceLast(x.name, into: first, begin: [0, 0, 0, 0], size: [1, shape[1], 1, half])
        let second = MILText.Tensor("\(name)_second", [1, shape[1], 1, half])
        program.sliceLast(x.name, into: second, begin: [0, 0, 0, half], size: [1, shape[1], 1, half])
        let negated = MILText.Tensor("\(name)_negated", [1, shape[1], 1, half])
        program.multiply(second.name, second.name, into: negated)
        let rotated = MILText.Tensor("\(name)_swapped", [1, shape[1], 1, headDim])
        program.concat([negated.name, first.name], into: rotated, axis: 3)
        let scaled = MILText.Tensor("\(name)_scaled", shape)
        program.multiply(x.name, cos, into: scaled)
        let turned = MILText.Tensor("\(name)_turned", shape)
        program.multiply(rotated.name, sin, into: turned)
        let out = MILText.Tensor("\(name)_out", shape)
        program.add(scaled.name, turned.name, into: out)
        return out
    }

    private func mlp(program: inout MILText, x: MILText.Tensor) -> MILText.Tensor {
        let intermediate = config.intermediateSize
        let gateUp = MILText.Tensor("gate_up", [1, 2 * intermediate])
        program.matmul(x.name, weights.gateUp, into: gateUp)
        let gate = MILText.Tensor("gate", [1, intermediate])
        program.sliceLast(gateUp.name, into: gate, begin: [0, 0], size: [1, intermediate])
        let up = MILText.Tensor("up", [1, intermediate])
        program.sliceLast(gateUp.name, into: up, begin: [0, intermediate], size: [1, intermediate])
        let activated = MILText.Tensor("activated", [1, intermediate])
        program.sigmoid(gate.name, into: activated)
        let gated = MILText.Tensor("gated", [1, intermediate])
        program.multiply(gate.name, activated.name, into: gated)
        let product = MILText.Tensor("product", [1, intermediate])
        program.multiply(gated.name, up.name, into: product)
        let out = MILText.Tensor("mlp_out", [1, hidden])
        program.matmul(product.name, weights.down, into: out)
        return out
    }
}
