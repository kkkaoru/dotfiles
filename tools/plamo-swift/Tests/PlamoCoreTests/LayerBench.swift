import CoreML
import Foundation
import Testing

@testable import PlamoCore

func note(_ text: String) {
    FileHandle.standardError.write(Data((text + "\n").utf8))
}

@Suite("layer bench", .serialized)
struct LayerBench {
    static let configJSON = """
        {
          "hidden_size": 1536,
          "num_hidden_layers": 16,
          "num_attention_heads": 16,
          "num_key_value_heads": 8,
          "head_dim": 128,
          "intermediate_size": 4096,
          "vocab_size": 100000,
          "rms_norm_eps": 1e-6,
          "window_size": 512,
          "sliding_window_pattern": 4,
          "rope_parameters": {
            "full_attention": { "rope_type": "default", "rope_theta": 1000000 },
            "sliding_attention": { "rope_type": "default", "rope_theta": 10000 }
          },
          "eos_token_id": [2],
          "bos_token_id": 1
        }
        """

    /// The layer's op count, and a prefix of it that still runs: bisection for a plan-builder crash.
    @Test("layer prefix")
    func prefix() throws {
        guard let countText = ProcessInfo.processInfo.environment["PLAMO_OPS"] else { return }
        let config = try JSONDecoder().decode(PlamoConfig.self, from: Data(Self.configJSON.utf8))
        let weights = DecoderLayerGraph.Weights(prefix: "model", layer: 3)
        let cacheLength = Int(ProcessInfo.processInfo.environment["PLAMO_CACHE"] ?? "64") ?? 64
        let graph = DecoderLayerGraph(config: config, weights: weights, cacheLength: cacheLength)
        let total = graph.buildProgram().opCount
        note("ops \(total)")
        if ProcessInfo.processInfo.environment["PLAMO_LIST"] != nil {
            let text = graph.buildProgram().render()
            let types = text.components(separatedBy: "type: \"").dropFirst()
            for (index, chunk) in types.enumerated() {
                let opType = chunk.components(separatedBy: "\"").first ?? ""
                let name = chunk.components(separatedBy: "name: \"").dropFirst().first?.components(separatedBy: "\"").first ?? ""
                note("\(index + 1) \(opType) \(name)")
            }
        }
        let requested = Int(countText) ?? total
        let ops = requested <= 0 || requested > total ? total : requested
        let program = graph.buildProgram(truncateTo: ops)
        let cache = FileManager.default.temporaryDirectory.appendingPathComponent("plamo-bench")
        let url = try ProgramCompiler(cache: cache).compile(program.render(), name: "prefix-\(ops)")
        note("compiled \(ops)")

        func random(_ shape: [Int], scale: Float) throws -> MLMultiArray {
            let count = shape.reduce(1, *)
            var values = [Float](repeating: 0, count: count)
            var generator = SystemRandomNumberGenerator()
            for index in 0..<count { values[index] = Float.random(in: -scale...scale, using: &generator) }
            return try MLMultiArray.fp16(values, shape: shape)
        }

        var inputs: [String: MLMultiArray] = [:]
        inputs["x"] = try random([1, 1, 1536], scale: 0.1)
        inputs["cos"] = try MLMultiArray.fp16([Float](repeating: 1, count: 128), shape: [1, 1, 1, 128])
        inputs["sin"] = try MLMultiArray.fp16([Float](repeating: 0, count: 128), shape: [1, 1, 1, 128])
        inputs["k_cache"] = try random([1, 8, cacheLength, 128], scale: 0.1)
        inputs["v_cache"] = try random([1, 8, cacheLength, 128], scale: 0.1)
        for weight in graph.weightInputs { inputs[weight.input] = try random(weight.shape, scale: 0.05) }

        note("predicting \(ops)")
        let runtime = try ProgramRuntime(url: url, computeUnits: .cpuOnly)
        note("loaded \(ops)")
        _ = try runtime.predict(inputs)
        note("RAN \(ops)")
    }

    @Test("one layer, timed")
    func bench() throws {
        // Off by default: it compiles a Core ML model and, until the BNNS crash is understood,
        // takes the whole test process down with it.
        guard ProcessInfo.processInfo.environment["PLAMO_BENCH"] != nil else { return }
        let config = try JSONDecoder().decode(PlamoConfig.self, from: Data(Self.configJSON.utf8))
        let weights = DecoderLayerGraph.Weights(prefix: "model", layer: 3)
        let cacheLength = Int(ProcessInfo.processInfo.environment["PLAMO_CACHE"] ?? "512") ?? 512
        note("cacheLength \(cacheLength)")
        let graph = DecoderLayerGraph(config: config, weights: weights, cacheLength: cacheLength)
        let cache = FileManager.default.temporaryDirectory.appendingPathComponent("plamo-bench")
        let url = try ProgramCompiler(cache: cache).compile(graph.render(), name: "layer")
        note("compiled \(url.lastPathComponent)")

        func random(_ shape: [Int], scale: Float) throws -> MLMultiArray {
            let count = shape.reduce(1, *)
            var values = [Float](repeating: 0, count: count)
            var generator = SystemRandomNumberGenerator()
            for index in 0..<count {
                values[index] = Float.random(in: -scale...scale, using: &generator)
            }
            return try .fp16(values, shape: shape)
        }

        var inputs: [String: MLMultiArray] = [:]
        inputs["x"] = try random([1, 1, 1536], scale: 0.1)
        inputs["cos"] = try .fp16([Float](repeating: 1, count: 128), shape: [1, 1, 1, 128])
        inputs["sin"] = try .fp16([Float](repeating: 0, count: 128), shape: [1, 1, 1, 128])
        inputs["k_cache"] = try random([1, 8, cacheLength, 128], scale: 0.1)
        inputs["v_cache"] = try random([1, 8, cacheLength, 128], scale: 0.1)
        note("x ok")
        for weight in graph.weightInputs {
            inputs[weight.input] = try random(weight.shape, scale: 0.05)
            note("input \(weight.input) \(weight.shape)")
        }

        let only = ProcessInfo.processInfo.environment["PLAMO_UNITS"]
        let unitsList: [MLComputeUnits] = [.cpuOnly, .cpuAndGPU, .cpuAndNeuralEngine]
        for units in unitsList where only == nil || "\(units.rawValue)" == only! {
            note("loading \(units.rawValue)")
            let runtime = try ProgramRuntime(url: url, computeUnits: units)
            note("loaded")
            _ = try runtime.predict(inputs)
            note("warmup ok")
            let steps = 20
            let start = Date()
            var out: [String: MLMultiArray] = [:]
            for _ in 0..<steps { out = try runtime.predict(inputs) }
            let per = Date().timeIntervalSince(start) / Double(steps) * 1000
            note(String(format: "%-20@ %6.2f ms/step  y[0]=%.4f", "\(units.rawValue)", per, out["y"]!.floats()[0]))
        }
    }
}
