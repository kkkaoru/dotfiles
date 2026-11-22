import Foundation
import MLX

/// One matrix as the safetensors store it: 4-bit affine, group 64. A checkpoint that ships plain
/// weights loads as `dense` instead, which is what the tests and an unquantized model use.
/// MLXArray is not Sendable, so neither is this: it lives inside the runtime actor, and only
/// completions cross that boundary.
public struct Quant {
    public static let groupSize = 64
    public static let bits = 4

    private enum Storage {
        case quantized(weight: MLXArray, scales: MLXArray, biases: MLXArray?)
        case dense(weight: MLXArray)
    }

    private let storage: Storage

    public init(arrays: [String: MLXArray], prefix: String) throws {
        guard let weight = arrays["\(prefix).weight"] else {
            throw PlamoError.missingModelFile("\(prefix).weight")
        }
        if let scales = arrays["\(prefix).scales"] {
            storage = .quantized(weight: weight, scales: scales, biases: arrays["\(prefix).biases"])
        } else {
            storage = .dense(weight: weight)
        }
    }

    public init(dense weight: MLXArray) {
        storage = .dense(weight: weight)
    }

    /// The dtype the activations arrive in: the quantized weights' scales, or a dense weight itself.
    public var activationDType: DType {
        switch storage {
        case .quantized(_, let scales, _): return scales.dtype
        case .dense(let weight): return weight.dtype
        }
    }

    public func matmul(_ x: MLXArray) -> MLXArray {
        switch storage {
        case .quantized(let weight, let scales, let biases):
            return MLX.quantizedMM(
                x, weight, scales: scales, biases: biases, transpose: true,
                groupSize: Quant.groupSize, bits: Quant.bits)
        case .dense(let weight):
            return x.matmul(weight.T)
        }
    }

    /// Quantized rows are dequantized after the gather, so only the tokens that were asked for are
    /// expanded rather than the whole vocabulary.
    public func embed(_ ids: MLXArray) -> MLXArray {
        switch storage {
        case .quantized(let weight, let scales, let biases):
            return MLX.dequantized(
                weight[ids], scales: scales[ids], biases: biases?[ids],
                groupSize: Quant.groupSize, bits: Quant.bits)
        case .dense(let weight):
            return weight[ids]
        }
    }
}
