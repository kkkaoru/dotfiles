import CoreML
import Foundation

/// A compiled ML Program, run with named fp16 inputs.
///
/// The decoder's graphs are functional: every value the step needs arrives as an input, and the
/// cache comes back as an output for Swift to keep. That keeps Core ML state out of the picture and
/// makes a layer's step testable against the MLX one it mirrors.
public final class ProgramRuntime {
    private let model: MLModel
    private let names: [String]

    /// Loads `url` with `computeUnits`; the Neural Engine path is `cpuAndNeuralEngine`.
    public init(url: URL, computeUnits: MLComputeUnits = .cpuAndNeuralEngine) throws {
        let configuration = MLModelConfiguration()
        configuration.computeUnits = computeUnits
        model = try MLModel(contentsOf: url, configuration: configuration)
        names = Array(model.modelDescription.inputDescriptionsByName.keys)
    }

    /// The inputs the graph expects, so a caller can fill exactly those.
    public var inputNames: [String] { names }

    /// Runs one step. A missing or misshapen input is Core ML's own error.
    public func predict(_ inputs: [String: MLMultiArray]) throws -> [String: MLMultiArray] {
        let provider = try MLDictionaryFeatureProvider(
            dictionary: inputs.mapValues { MLFeatureValue(multiArray: $0) })
        let out = try model.prediction(from: provider)
        var result: [String: MLMultiArray] = [:]
        for name in out.featureNames {
            if let value = out.featureValue(for: name)?.multiArrayValue {
                result[name] = value
            }
        }
        return result
    }
}

extension MLMultiArray {
    /// An fp16 array over `shape`, filled from `values` in row-major order.
    public static func fp16(_ values: [Float], shape: [Int]) throws -> MLMultiArray {
        let array = try MLMultiArray(
            shape: shape.map { NSNumber(value: $0) }, dataType: .float16)
        let pointer = array.dataPointer.bindMemory(to: Float16.self, capacity: values.count)
        for (index, value) in values.enumerated() {
            pointer[index] = Float16(value)
        }
        return array
    }

    /// The array's values in row-major order.
    public func floats() -> [Float] {
        let count = shape.map { $0.intValue }.reduce(1, *)
        let pointer = dataPointer.bindMemory(to: Float16.self, capacity: count)
        return (0..<count).map { Float(pointer[$0]) }
    }
}
