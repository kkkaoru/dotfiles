import CoreML
import Foundation
import Testing

@testable import PlamoCore

/// One op family at a time, so a crash names the op that caused it. Run with `PLAMO_OP=<case>`.
@Suite("op probe", .serialized)
struct OpProbe {
    @Test("probe")
    func probe() throws {
        guard let name = ProcessInfo.processInfo.environment["PLAMO_OP"] else { return }
        var program = MILText()
        let x = MILText.Tensor("x", [1, 2, 4, 8])
        program.addInput(x)
        guard let out = build(name: name, program: &program, x: x) else {
            Issue.record("unknown case \(name)")
            return
        }
        program.setOutputs([out])

        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("plamo-probe")
        let url = try ProgramCompiler(cache: directory).compile(program.render(), name: name)
        let runtime = try ProgramRuntime(url: url, computeUnits: .cpuOnly)
        let result = try runtime.predict(try inputs(name: name, x: x))
        let values = result[out.name]!.floats()
        let line = "OK \(name) first=\(values[0]) count=\(values.count)\n"
        FileHandle.standardError.write(Data(line.utf8))
    }

    /// The case's graph, or nil when the name is not a case.
    private func build(
        name: String, program: inout MILText, x: MILText.Tensor
    ) -> MILText.Tensor? {
        if let out = shapes(name: name, program: &program, x: x) { return out }
        if let out = norms(name: name, program: &program, x: x) { return out }
        if let out = headNorms(name: name, program: &program) { return out }
        return arithmetic(name: name, program: &program, x: x)
    }

    /// Slicing, joining and re-viewing a tensor: the cache trim is the one that matters.
    private func shapes(
        name: String, program: inout MILText, x: MILText.Tensor
    ) -> MILText.Tensor? {
        switch name {
        case "slice":
            let out = MILText.Tensor("sliced", [1, 2, 4, 4])
            program.sliceLast(x.name, into: out, begin: [0, 0, 0, 2], size: [1, 2, 4, 4])
            return out
        case "sliceMid":
            let out = MILText.Tensor("sliced", [1, 2, 2, 8])
            program.sliceLast(x.name, into: out, begin: [0, 0, 1, 0], size: [1, 2, 2, 8])
            return out
        case "concat":
            let out = MILText.Tensor("joined", [1, 2, 8, 8])
            program.concat([x.name, x.name], into: out, axis: 2)
            return out
        case "concat3":
            let out = MILText.Tensor("joined", [1, 2, 5, 8])
            let tail = MILText.Tensor("tail", [1, 2, 1, 8])
            program.addInput(tail)
            program.concat([x.name, tail.name], into: out, axis: 2)
            return out
        case "concatSlice":
            // The layer slices the concat's result to drop the oldest row, not an input.
            let joined = MILText.Tensor("joined", [1, 2, 5, 8])
            program.concat([x.name, x.name], into: joined, axis: 2)
            let out = MILText.Tensor("trimmed", [1, 2, 4, 8])
            program.sliceLast(joined.name, into: out, begin: [0, 0, 1, 0], size: [1, 2, 4, 8])
            return out
        case "transpose":
            let out = MILText.Tensor("transposed", [1, 2, 8, 4])
            program.transpose(x.name, into: out, perm: [0, 1, 3, 2])
            return out
        case "reshape":
            let out = MILText.Tensor("reshaped", [1, 8, 8])
            program.reshape(x.name, into: out, shape: out.shape)
            return out
        default:
            return nil
        }
    }

    /// The RMSNorm shapes the layer uses, including the transpose that reaches them.
    private func norms(
        name: String, program: inout MILText, x: MILText.Tensor
    ) -> MILText.Tensor? {
        switch name {
        case "norm":
            let squares = MILText.Tensor("squares", x.shape)
            program.multiply(x.name, x.name, into: squares)
            let mean = MILText.Tensor("mean", [1, 2, 4, 1])
            program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
            let inverse = MILText.Tensor("inverse", [1, 2, 4, 1])
            program.rsqrt(mean.name, into: inverse, epsilon: 1e-6)
            let scaled = MILText.Tensor("scaled", x.shape)
            program.multiply(x.name, inverse.name, into: scaled)
            let weight = MILText.Tensor("weight", [1, 2, 4, 8])
            program.addInput(weight)
            let out = MILText.Tensor("normed", x.shape)
            program.multiply(scaled.name, weight.name, into: out)
            return out
        default:
            return nil
        }
    }

    /// The per-head norm at rank 4, and the two ways the layer reaches its shape.
    private func headNorms(name: String, program: inout MILText) -> MILText.Tensor? {
        switch name {
        case "norm4":
            // The per-head norm: rank 4 in, and the mean keeps a degenerate [1, 16, 1, 1] shape so
            // it broadcasts back over the head axis.
            let heads = MILText.Tensor("heads", [1, 16, 1, 128])
            program.addInput(heads)
            let squares = MILText.Tensor("squares", [1, 16, 1, 128])
            program.multiply(heads.name, heads.name, into: squares)
            let mean = MILText.Tensor("mean", [1, 16, 1, 1])
            program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
            let inverse = MILText.Tensor("inverse", [1, 16, 1, 1])
            program.rsqrt(mean.name, into: inverse, epsilon: 1e-6)
            let out = MILText.Tensor("normed", [1, 16, 1, 128])
            program.multiply(heads.name, inverse.name, into: out)
            return out
        case "transposeNorm":
            // The layer's per-head norm, reached the way the layer reaches it: reshape, transpose,
            // square, reduce. `norm4` does the same at the same shape on a model input and passes,
            // so the transpose (or the reshape before it) is what the plan builder faults on.
            let flatT = MILText.Tensor("flat_t", [1, 1, 16, 128])
            program.addInput(flatT)
            let movedT = MILText.Tensor("moved_t", [1, 16, 1, 128])
            program.transpose(flatT.name, into: movedT, perm: [0, 2, 1, 3])
            let squaresT = MILText.Tensor("squares_t", [1, 16, 1, 128])
            program.multiply(movedT.name, movedT.name, into: squaresT)
            let meanT = MILText.Tensor("mean_t", [1, 16, 1, 1])
            program.reduceMean(squaresT.name, into: meanT, axes: [-1], keepDims: true)
            let out = MILText.Tensor("inverse_t", [1, 16, 1, 1])
            program.rsqrt(meanT.name, into: out, epsilon: 1e-6)
            return out
        case "reshapeInput":
            // The layer's RMSNorm reshapes its rank-1 weight to [1, 1_536] before the multiply. A
            // reshape of a *model input* is the one shape in the crashing prefix that no other
            // case covers, and Core ML is free to fold it into the input's binding.
            let flatR = MILText.Tensor("flat_r", [1, 1_536])
            program.addInput(flatR)
            let weightR = MILText.Tensor("weight_r", [1_536])
            program.addInput(weightR)
            let shapedR = MILText.Tensor("weight_r_shaped", [1, 1_536])
            program.reshape(weightR.name, into: shapedR, shape: [1, 1_536])
            let out = MILText.Tensor("scaled_r", [1, 1_536])
            program.multiply(flatR.name, shapedR.name, into: out)
            return out
        case "norm2":
            // The layer's RMSNorm on a rank-2 hidden state: the mean keeps its dims as [1, 1] and
            // is broadcast back over [1, 1_536].
            let flat = MILText.Tensor("flat", [1, 1_536])
            program.addInput(flat)
            let squares = MILText.Tensor("squares", [1, 1_536])
            program.multiply(flat.name, flat.name, into: squares)
            let mean = MILText.Tensor("mean", [1, 1])
            program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
            let inverse = MILText.Tensor("inverse", [1, 1])
            program.rsqrt(mean.name, into: inverse, epsilon: 1e-6)
            let scaled = MILText.Tensor("scaled", [1, 1_536])
            program.multiply(flat.name, inverse.name, into: scaled)
            let out = MILText.Tensor("normed", [1, 1_536])
            program.multiply(scaled.name, scaled.name, into: out)
            return out
        default:
            return nil
        }
    }

    /// The matmuls at the decoder's real widths, plus the two elementwise ops.
    private func arithmetic(
        name: String, program: inout MILText, x: MILText.Tensor
    ) -> MILText.Tensor? {
        switch name {
        case "gateup":
            // The widest op in the decoder at its real size: [1, 1_536] @ [1_536, 8_192]. The names
            // must not be the suite's own `x`, or the model gets two inputs called x.
            let wide = MILText.Tensor("wide", [1, 1_536])
            program.addInput(wide)
            let w = MILText.Tensor("w", [1_536, 8_192])
            program.addInput(w)
            let out = MILText.Tensor("gate_up", [1, 8_192])
            program.matmul(wide.name, w.name, into: out)
            return out
        case "weights10":
            // Every weight the layer graph declares, at its real size, with only one of them read.
            // Truncating the graph never removed these, so a crash that survives every prefix can
            // still be about binding them.
            let flat = MILText.Tensor("flat10", [1, 1_536])
            program.addInput(flat)
            var names: [String] = []
            for index in 0..<10 {
                let weight = MILText.Tensor("w\(index)", [1_536, 8_192])
                program.addInput(weight)
                names.append(weight.name)
            }
            let out = MILText.Tensor("wide_out", [1, 8_192])
            program.matmul(flat.name, names[0], into: out)
            return out
        case "matmul3":
            let a = MILText.Tensor("a", [2, 4, 8])
            program.addInput(a)
            let b = MILText.Tensor("b", [2, 8, 4])
            program.addInput(b)
            let out = MILText.Tensor("product", [2, 4, 4])
            program.matmul(a.name, b.name, into: out)
            return out
        case "matmul4":
            let a = MILText.Tensor("a", [1, 2, 4, 8])
            program.addInput(a)
            let out = MILText.Tensor("product", [1, 2, 4, 4])
            program.matmul(a.name, x.name, into: out)
            return out
        case "scalar":
            let out = MILText.Tensor("scaled", x.shape)
            program.multiplyScalar(x.name, into: out, scale: 0.25)
            return out
        case "softmax":
            let out = MILText.Tensor("probs", x.shape)
            program.softmax(x.name, into: out, axis: -1)
            return out
        default:
            return nil
        }
    }

    /// The case's own inputs, beside the suite's `x`.
    private func inputs(name: String, x: MILText.Tensor) throws -> [String: MLMultiArray] {
        var inputs: [String: MLMultiArray] = [
            x.name: try MLMultiArray.fp16((0..<64).map { Float($0) / 64 }, shape: x.shape)
        ]
        for (key, array) in try smallInputs(name: name) { inputs[key] = array }
        for (key, array) in try largeInputs(name: name) { inputs[key] = array }
        return inputs
    }

    private func smallInputs(name: String) throws -> [String: MLMultiArray] {
        switch name {
        case "norm":
            return ["weight": try ones(64, [1, 2, 4, 8])]
        case "matmul3":
            return ["a": try ones(64, [2, 4, 8]), "b": try ones(64, [2, 8, 4])]
        case "concat3":
            return ["tail": try ones(16, [1, 2, 1, 8])]
        case "matmul4":
            return ["a": try ones(64, [1, 2, 4, 8])]
        default:
            return [:]
        }
    }

    private func largeInputs(name: String) throws -> [String: MLMultiArray] {
        switch name {
        case "gateup":
            return [
                "wide": try ramp(1_536, [1, 1_536]),
                "w": try MLMultiArray.fp16(
                    (0..<(1_536 * 8_192)).map { Float($0 % 97) / 97 }, shape: [1_536, 8_192]),
            ]
        case "weights10":
            var inputs: [String: MLMultiArray] = ["flat10": try ramp(1_536, [1, 1_536])]
            for index in 0..<10 {
                inputs["w\(index)"] = try MLMultiArray.fp16(
                    [Float](repeating: 0.5, count: 1_536 * 8_192), shape: [1_536, 8_192])
            }
            return inputs
        case "transposeNorm":
            return ["flat_t": try ramp(16 * 128, [1, 1, 16, 128])]
        case "reshapeInput":
            return ["flat_r": try ramp(1_536, [1, 1_536]), "weight_r": try ones(1_536, [1_536])]
        case "norm4":
            return ["heads": try ramp(16 * 128, [1, 16, 1, 128])]
        case "norm2":
            return ["flat": try ramp(1_536, [1, 1_536])]
        default:
            return [:]
        }
    }

    /// A constant fp16 array, so a case states a shape and its values on one short line.
    private func ones(_ count: Int, _ shape: [Int]) throws -> MLMultiArray {
        try MLMultiArray.fp16([Float](repeating: 1, count: count), shape: shape)
    }

    private func ramp(_ count: Int, _ shape: [Int]) throws -> MLMultiArray {
        try MLMultiArray.fp16((0..<count).map { Float($0) / Float(count) }, shape: shape)
    }
}
