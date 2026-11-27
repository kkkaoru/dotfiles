import CoreML
import Foundation
import MLX

/// plamo3's whole decoder as chained Core ML blocks, driven from Swift with no Python anywhere.
///
/// The 89-op layer model is compiled once and serves every layer: its weights are model inputs, so
/// Swift binds each layer's tensors and carries the trimmed caches from block to block. That keeps
/// the plan small enough to run (a 1400-op plan tripped a composition-dependent BNNS fault) while
/// still executing every operation of the decoder inside Core ML — the two ends included, since the
/// embedding is a row copy and the final norm plus lm_head is a second small model.
public struct CoreMLDecoder {
    /// The constants every block binds: a single `-1`, the query scale, and the mask's dead value.
    private enum Constant {
        static let negOne = "neg_one"
        static let scale = "attn_scale"
        static let mask = "mask"
        /// Large enough that a softmax weight over a dead row underflows to zero in fp16.
        static let maskedScore: Float = -30_000
    }

    private let config: PlamoConfig
    private let layer: ProgramRuntime
    private let head: ProgramRuntime
    private let layerWeights: [[String: MLMultiArray]]
    private let embedding: MLMultiArray
    private let finalNorm: MLMultiArray
    private let headWeight: MLMultiArray
    private let rotary: [Rotary]
    private let cacheLength: Int
    private let negOne: MLMultiArray
    private let scale: MLMultiArray
    private var keys: [MLMultiArray]
    private var values: [MLMultiArray]
    /// How many tokens this decoder has consumed, which is what the mask is derived from.
    private var seen = 0

    /// The shape of one cache: `[1, kvHeads, cacheLength, headDim]`.
    private var cacheShape: [Int] {
        [1, config.numKeyValueHeads, cacheLength, config.headDim]
    }

    /// Loads the checkpoint and compiles the two models. `cache` holds the compiled artifacts.
    ///
    /// The cache is sized to the sliding window, which is what a sliding layer trims at; a full
    /// layer keeps the same length, so a prompt longer than the window is not served exactly.
    public init(
        directory: URL, computeUnits: MLComputeUnits = .cpuAndNeuralEngine, cache: URL? = nil
    ) throws {
        let config = try PlamoConfig.load(directory: directory)
        let checkpoint = directory.appendingPathComponent("model.safetensors")
        let arrays = try loadArrays(url: checkpoint)
        let cacheLength = config.windowSize
        let cacheDirectory =
            cache
            ?? FileManager.default.temporaryDirectory.appendingPathComponent("plamo-decoder")
        let compiler = ProgramCompiler(cache: cacheDirectory)

        self.config = config
        self.cacheLength = cacheLength
        let layerText = try Self.layerText(config: config, index: 0, cacheLength: cacheLength)
        self.layer = try ProgramRuntime(
            url: compiler.compile(layerText, name: "decoder-layer"), computeUnits: computeUnits)
        self.head = try ProgramRuntime(
            url: compiler.compile(Self.headProgram(config: config).render(), name: "decoder-head"),
            computeUnits: computeUnits)
        self.layerWeights = try (0..<config.numHiddenLayers).map { index in
            try Self.bindLayer(
                config: config, index: index, cacheLength: cacheLength, arrays: arrays)
        }
        self.embedding = try Self.tensor(
            "model.embed_tokens.weight", shape: [config.vocabSize, config.hiddenSize],
            arrays: arrays)
        self.finalNorm = try Self.tensor(
            "model.norm.weight", shape: [config.hiddenSize], arrays: arrays)
        self.headWeight = try Self.tensor(
            "lm_head.weight", shape: [config.vocabSize, config.hiddenSize], arrays: arrays)
        self.rotary = try (0..<config.numHiddenLayers).map { index in
            Rotary(try Self.ropeParameters(config: config, index: index), headDim: config.headDim)
        }
        self.negOne = try MLMultiArray.fp16([-1], shape: [1])
        self.scale = try MLMultiArray.fp16([1 / Float(config.headDim).squareRoot()], shape: [1])
        let zero = try MLMultiArray(
            shape: [1, config.numKeyValueHeads, cacheLength, config.headDim].map {
                NSNumber(value: $0)
            }, dataType: .float16)
        self.keys = Array(repeating: zero, count: config.numHiddenLayers)
        self.values = Array(repeating: zero, count: config.numHiddenLayers)
    }

    /// Drops the cache, so the next token starts a new sequence.
    public mutating func reset() {
        let zero = keys[0]
        keys = Array(repeating: zero, count: config.numHiddenLayers)
        values = Array(repeating: zero, count: config.numHiddenLayers)
        seen = 0
    }

    /// One decode step: the logits the model gives the token after `token`.
    public mutating func logits(forToken token: Int) throws -> [Float] {
        var hidden = try embeddingRow(token)
        let mask = try maskArray()
        let tables = try (0..<layerWeights.count).map { index in
            try ropeTable(layer: index, position: seen)
        }
        for index in 0..<layerWeights.count {
            var inputs = layerWeights[index]
            inputs["x"] = hidden
            inputs["cos"] = tables[index].0
            inputs["sin"] = tables[index].1
            inputs[Constant.negOne] = negOne
            inputs[Constant.scale] = scale
            inputs[Constant.mask] = mask
            inputs["k_cache"] = keys[index]
            inputs["v_cache"] = values[index]
            let output = try layer.predict(inputs)
            guard let y = output["y"], let keptKeys = output["keys_trimmed"],
                let keptValues = output["values_trimmed"]
            else { throw PlamoError.missingModelFile("decoder layer outputs") }
            hidden = y
            keys[index] = keptKeys
            values[index] = keptValues
        }
        seen += 1
        return try headLogits(hidden)
    }

    /// The greedy token for `token`: the argmax of the logits.
    public mutating func nextToken(after token: Int) throws -> Int {
        let logits = try logits(forToken: token)
        guard let best = logits.indices.max(by: { logits[$0] < logits[$1] }) else {
            throw PlamoError.missingModelFile("logits")
        }
        return best
    }

    /// The embedding row for `token`, as the graph's `[1, hidden]` input.
    private func embeddingRow(_ token: Int) throws -> MLMultiArray {
        let hidden = config.hiddenSize
        guard token >= 0, token < config.vocabSize else {
            throw PlamoError.invalidConfig(field: "token")
        }
        let source = embedding.dataPointer.bindMemory(
            to: UInt16.self, capacity: config.vocabSize * hidden)
        let row = try MLMultiArray(
            shape: [1, 1, hidden].map { NSNumber(value: $0) }, dataType: .float16)
        let destination = row.dataPointer.bindMemory(to: UInt16.self, capacity: hidden)
        for index in 0..<hidden { destination[index] = source[token * hidden + index] }
        return row
    }

    /// Zero for a cache row that holds a token, the dead value for a row not reached yet.
    ///
    /// The trim appends at the end and drops from the front, so the rows holding tokens are the
    /// last `seen + 1` of the cache.
    private func maskArray() throws -> MLMultiArray {
        let valid = min(seen + 1, cacheLength)
        var values = [Float](repeating: Constant.maskedScore, count: cacheLength)
        for index in (cacheLength - valid)..<cacheLength { values[index] = 0 }
        return try MLMultiArray.fp16(values, shape: [1, 1, cacheLength])
    }

    /// The RoPE tables for one layer at `position`, shaped as the graph's inputs.
    private func ropeTable(layer index: Int, position: Int) throws -> (MLMultiArray, MLMultiArray) {
        let (cos, sin) = try rotary[index].cosSin(positions: [position])
        let shape = [1, 1, 1, config.headDim]
        return (try Self.half(cos, shape: shape), try Self.half(sin, shape: shape))
    }

    /// The final norm and lm_head, which turn the last block's hidden state into logits.
    private func headLogits(_ hidden: MLMultiArray) throws -> [Float] {
        let output = try head.predict([
            "h": hidden, "w_final_norm": finalNorm, "w_lm_head": headWeight,
        ])
        guard let logits = output["logits"] else {
            throw PlamoError.missingModelFile("logits")
        }
        return logits.floats()
    }
}

extension CoreMLDecoder {
    /// The layer program's text, which is the same for every layer: only the weight names differ.
    internal static func layerText(
        config: PlamoConfig, index: Int, cacheLength: Int
    ) throws -> String {
        DecoderLayerGraph(
            config: config, weights: .init(prefix: "model", layer: index), cacheLength: cacheLength
        ).render()
    }

    /// The tensors one layer binds, keyed by the graph's input name.
    internal static func bindLayer(
        config: PlamoConfig, index: Int, cacheLength: Int, arrays: [String: MLXArray]
    ) throws -> [String: MLMultiArray] {
        let graph = DecoderLayerGraph(
            config: config, weights: .init(prefix: "model", layer: index), cacheLength: cacheLength)
        var bound: [String: MLMultiArray] = [:]
        for weight in graph.weightInputs {
            bound[weight.input] = try tensor(weight.checkpoint, shape: weight.shape, arrays: arrays)
        }
        return bound
    }

    /// One checkpoint tensor as the graph declares it: dequantized and in fp16.
    ///
    /// The weight keeps the checkpoint's own `[out, in]` order; the graph transposes it inside the
    /// matmul. Transposing here meant reading a `swappedAxes` view, which made MLX gather every
    /// element: 11.1 s per weight against 0.032 s for the same bytes untransposed.
    internal static func tensor(
        _ checkpoint: String, shape: [Int], arrays: [String: MLXArray]
    ) throws -> MLMultiArray {
        guard let packed = arrays[checkpoint] else {
            throw PlamoError.missingModelFile(checkpoint)
        }
        var value = packed
        let base =
            checkpoint.hasSuffix(".weight")
            ? String(checkpoint.dropLast(".weight".count)) : checkpoint
        if let scales = arrays[base + ".scales"], let biases = arrays[base + ".biases"] {
            value = dequantized(packed, scales: scales, biases: biases, groupSize: 64, bits: 4)
        }
        return try half(value, shape: shape)
    }

    /// An MLX tensor as an fp16 `MLMultiArray`, copied as raw bytes.
    ///
    /// Going through `asArray` and back took minutes for the whole checkpoint: it materializes a
    /// Swift `[Float]` of every element. The bytes are already fp16, so they are copied directly.
    internal static func half(_ value: MLXArray, shape: [Int]) throws -> MLMultiArray {
        let half = value.asType(.float16)
        let result = try MLMultiArray(
            shape: shape.map { NSNumber(value: $0) }, dataType: .float16)
        let bytes = half.asData(access: .copy).data
        guard bytes.count == shape.reduce(1, *) * MemoryLayout<UInt16>.size else {
            throw PlamoError.invalidConfig(field: "tensor shape")
        }
        bytes.withUnsafeBytes { source in
            guard let address = source.baseAddress else { return }
            result.dataPointer.copyMemory(from: address, byteCount: bytes.count)
        }
        return result
    }

    /// The RoPE parameters a layer uses: sliding layers take the sliding table.
    internal static func ropeParameters(
        config: PlamoConfig, index: Int
    ) throws -> PlamoRopeParameters {
        let sliding = !(index + 1).isMultiple(of: max(config.slidingWindowPattern, 1))
        guard
            let parameters = config.ropeParameters[sliding ? "sliding_attention" : "full_attention"]
        else { throw PlamoError.invalidConfig(field: "rope_parameters") }
        return parameters
    }

    /// The final RMSNorm and the lm_head matmul as one small program.
    internal static func headProgram(config: PlamoConfig) -> MILText {
        var program = MILText()
        let hidden = config.hiddenSize
        let vocab = config.vocabSize
        let h = MILText.Tensor("h", [1, 1, hidden])
        let weight = MILText.Tensor("w_final_norm", [hidden])
        let lmHead = MILText.Tensor("w_lm_head", [vocab, hidden])
        for tensor in [h, weight, lmHead] { program.addInput(tensor) }

        // The decoder's last layer yields [1, 1, hidden]; the norm and lm_head work on one row.
        let flat = MILText.Tensor("h_flat", [1, hidden])
        program.reshape(h.name, into: flat, shape: [1, hidden])
        let squares = MILText.Tensor("h_squares", [1, hidden])
        program.multiply(flat.name, flat.name, into: squares)
        let mean = MILText.Tensor("h_mean", [1, 1])
        program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
        let inverse = MILText.Tensor("h_inverse", [1, 1])
        program.rsqrt(mean.name, into: inverse, epsilon: Double(config.rmsNormEps))
        let scaled = MILText.Tensor("h_scaled", [1, hidden])
        program.multiply(flat.name, inverse.name, into: scaled)
        let shaped = MILText.Tensor("w_final_norm_shaped", [1, hidden])
        program.reshape(weight.name, into: shaped, shape: [1, hidden])
        let normed = MILText.Tensor("normed", [1, hidden])
        program.multiply(scaled.name, shaped.name, into: normed)
        let logits = MILText.Tensor("logits", [1, vocab])
        program.matmul(normed.name, lmHead.name, into: logits, transposeY: true)
        program.setOutputs([logits])
        return program
    }
}
