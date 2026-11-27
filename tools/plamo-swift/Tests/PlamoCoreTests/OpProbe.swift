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
        var out = x
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
            out = MILText.Tensor("normed", x.shape)
            program.multiply(scaled.name, weight.name, into: out)
        case "slice":
            out = MILText.Tensor("sliced", [1, 2, 4, 4])
            program.sliceLast(x.name, into: out, begin: [0, 0, 0, 2], size: [1, 2, 4, 4])
        case "sliceMid":
            out = MILText.Tensor("sliced", [1, 2, 2, 8])
            program.sliceLast(x.name, into: out, begin: [0, 0, 1, 0], size: [1, 2, 2, 8])
        case "concat":
            out = MILText.Tensor("joined", [1, 2, 8, 8])
            program.concat([x.name, x.name], into: out, axis: 2)
        case "concat3":
            out = MILText.Tensor("joined", [1, 2, 5, 8])
            let tail = MILText.Tensor("tail", [1, 2, 1, 8])
            program.addInput(tail)
            program.concat([x.name, tail.name], into: out, axis: 2)
        case "gateup":
            // The widest op in the decoder at its real size: [1, 1536] @ [1536, 8192].
            let x = MILText.Tensor("x", [1, 1536])
            program.addInput(x)
            let w = MILText.Tensor("w", [1536, 8192])
            program.addInput(w)
            out = MILText.Tensor("gate_up", [1, 8192])
            program.matmul(x.name, w.name, into: out)
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
            out = MILText.Tensor("normed", [1, 16, 1, 128])
            program.multiply(heads.name, inverse.name, into: out)
        case "norm2":
            // The layer's RMSNorm on a rank-2 hidden state: the mean keeps its dims as [1, 1] and
            // is broadcast back over [1, 1536].
            let flat = MILText.Tensor("flat", [1, 1536])
            program.addInput(flat)
            let squares = MILText.Tensor("squares", [1, 1536])
            program.multiply(flat.name, flat.name, into: squares)
            let mean = MILText.Tensor("mean", [1, 1])
            program.reduceMean(squares.name, into: mean, axes: [-1], keepDims: true)
            let inverse = MILText.Tensor("inverse", [1, 1])
            program.rsqrt(mean.name, into: inverse, epsilon: 1e-6)
            let scaled = MILText.Tensor("scaled", [1, 1536])
            program.multiply(flat.name, inverse.name, into: scaled)
            out = MILText.Tensor("normed", [1, 1536])
            program.multiply(scaled.name, scaled.name, into: out)
        case "concatSlice":
            // The layer slices the concat's result to drop the oldest row, not an input.
            let joined = MILText.Tensor("joined", [1, 2, 5, 8])
            program.concat([x.name, x.name], into: joined, axis: 2)
            out = MILText.Tensor("trimmed", [1, 2, 4, 8])
            program.sliceLast(joined.name, into: out, begin: [0, 0, 1, 0], size: [1, 2, 4, 8])
        case "transpose":
            out = MILText.Tensor("transposed", [1, 2, 8, 4])
            program.transpose(x.name, into: out, perm: [0, 1, 3, 2])
        case "softmax":
            out = MILText.Tensor("probs", x.shape)
            program.softmax(x.name, into: out, axis: -1)
        case "scalar":
            out = MILText.Tensor("scaled", x.shape)
            program.multiplyScalar(x.name, into: out, scale: 0.25)
        case "matmul3":
            let a = MILText.Tensor("a", [2, 4, 8])
            program.addInput(a)
            out = MILText.Tensor("product", [2, 4, 4])
            program.matmul(a.name, MILText.Tensor("b", [2, 8, 4]).name, into: out)
            program.addInput(MILText.Tensor("b", [2, 8, 4]))
        case "matmul4":
            let a = MILText.Tensor("a", [1, 2, 4, 8])
            program.addInput(a)
            out = MILText.Tensor("product", [1, 2, 4, 4])
            program.matmul(a.name, x.name, into: out)
        case "reshape":
            out = MILText.Tensor("reshaped", [1, 8, 8])
            program.reshape(x.name, into: out, shape: out.shape)
        default:
            Issue.record("unknown case \(name)")
            return
        }
        program.setOutputs([out])

        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("plamo-probe")
        let url = try ProgramCompiler(cache: directory).compile(program.render(), name: name)
        var inputs: [String: MLMultiArray] = [
            x.name: try MLMultiArray.fp16((0..<64).map { Float($0) / 64 }, shape: x.shape),
        ]
        if name == "norm" {
            inputs["weight"] = try MLMultiArray.fp16([Float](repeating: 1, count: 64), shape: [1, 2, 4, 8])
        }
        if name == "matmul3" {
            inputs["a"] = try MLMultiArray.fp16([Float](repeating: 1, count: 64), shape: [2, 4, 8])
            inputs["b"] = try MLMultiArray.fp16([Float](repeating: 1, count: 64), shape: [2, 8, 4])
        }
        if name == "gateup" {
            inputs["x"] = try MLMultiArray.fp16((0..<1536).map { Float($0) / 1536 }, shape: [1, 1536])
            inputs["w"] = try MLMultiArray.fp16(
                (0..<(1536 * 8192)).map { Float($0 % 97) / 97 }, shape: [1536, 8192])
        }
        if name == "norm4" {
            inputs["heads"] = try MLMultiArray.fp16((0..<(16 * 128)).map { Float($0) / 2048 }, shape: [1, 16, 1, 128])
        }
        if name == "norm2" {
            inputs["flat"] = try MLMultiArray.fp16((0..<1536).map { Float($0) / 1536 }, shape: [1, 1536])
        }
        if name == "concat3" {
            inputs["tail"] = try MLMultiArray.fp16([Float](repeating: 1, count: 16), shape: [1, 2, 1, 8])
        }
        if name == "matmul4" {
            inputs["a"] = try MLMultiArray.fp16([Float](repeating: 1, count: 64), shape: [1, 2, 4, 8])
        }
        let runtime = try ProgramRuntime(url: url, computeUnits: .cpuOnly)
        let result = try runtime.predict(inputs)
        let values = result[out.name]!.floats()
        FileHandle.standardError.write(Data("OK \(name) first=\(values[0]) count=\(values.count)\n".utf8))
    }
}
