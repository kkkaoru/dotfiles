import Foundation

/// Emits a Core ML ML Program as protobuf **text**, which `protoc --encode` turns into a
/// `model.mlmodel` and `coremlc` compiles. This exists because the only usual way to author an ML
/// Program is coremltools, which is Python: here the graph is written by Swift and the toolchain is
/// a C++ encoder plus Xcode's compiler.
///
/// The text is deliberately the wire schema, not a DSL: `Model.mlProgram` is `MILSpec.Program`, so
/// every value carries its `tensorType` and every `matmul` takes `transpose_x`/`transpose_y` as
/// `BOOL` inputs (attributes are rejected by the validator). `specificationVersion` must be at least
/// 7 for `FLOAT16` multi-array features.
public struct MILText {
    /// One named tensor: the decoder's values are all fp16 tensors of a known rank.
    public struct Tensor: Equatable {
        public let name: String
        public let shape: [Int]
        public init(_ name: String, _ shape: [Int]) {
            self.name = name
            self.shape = shape
        }

        var typeText: String {
            let dimensions = shape.map { "dimensions { constant { size: \($0) } }" }.joined(separator: " ")
            return "tensorType { dataType: FLOAT16 rank: \(shape.count) \(dimensions) }"
        }
    }

    /// A model input, a constant or an operation output.
    public private(set) var inputs: [Tensor] = []
    private var operations: [String] = []
    private var outputs: [Tensor] = []

    public init() {}

    public mutating func addInput(_ tensor: Tensor) {
        inputs.append(tensor)
    }

    public mutating func setOutputs(_ tensors: [Tensor]) {
        outputs = tensors
    }

    /// `y = x @ w`, with both transposes spelled out as the validator requires.
    public mutating func matmul(_ x: String, _ w: String, into y: Tensor) {
        emit(
            type: "matmul",
            inputs: [("x", binding(x)), ("y", binding(w))],
            extraInputs: [("transpose_x", bool(false)), ("transpose_y", bool(false))],
            output: y
        )
    }

    public mutating func add(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "add", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    public mutating func multiply(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "mul", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    public mutating func softmax(_ x: String, into y: Tensor, axis: Int) {
        emit(
            type: "softmax",
            inputs: [("x", binding(x))],
            extraInputs: [("axis", integer(axis))],
            output: y
        )
    }

    /// `y = a @ concat([b, c])`-free helper: joins two tensors along `axis`.
    public mutating func concat(_ tensors: [String], into y: Tensor, axis: Int) {
        emit(
            type: "concat",
            inputs: [("values", tensors.map { "arguments { name: \"\($0)\" }" }.joined(separator: " "))],
            extraInputs: [("axis", integer(axis)), ("interleave", bool(false))],
            output: y
        )
    }

    public mutating func subtract(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "sub", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    public mutating func divide(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "real_div", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    /// `x + eps`, for RMSNorm's denominator.
    public mutating func addScalar(_ x: String, into y: Tensor, epsilon: Double) {
        emit(type: "add", inputs: [("x", binding(x)), ("y", halfScalar(epsilon))], output: y)
    }

    /// `x * k`, for the attention scale.
    public mutating func multiplyScalar(_ x: String, into y: Tensor, scale: Double) {
        emit(type: "mul", inputs: [("x", binding(x)), ("y", halfVector(scale))], output: y)
    }

    public mutating func rsqrt(_ x: String, into y: Tensor, epsilon: Double) {
        emit(type: "rsqrt", inputs: [("x", binding(x)), ("epsilon", halfScalar(epsilon))], output: y)
    }

    public mutating func sigmoid(_ x: String, into y: Tensor) {
        emit(type: "sigmoid", inputs: [("x", binding(x))], output: y)
    }

    /// RMSNorm's mean of squares; `axes` is a constant list of the reduced dimensions.
    public mutating func reduceMean(_ x: String, into y: Tensor, axes: [Int], keepDims: Bool) {
        emit(
            type: "reduce_mean",
            inputs: [("x", binding(x))],
            extraInputs: [("axes", ints(axes)), ("keep_dims", bool(keepDims))],
            output: y
        )
    }

    public mutating func reshape(_ x: String, into y: Tensor, shape: [Int]) {
        emit(
            type: "reshape", inputs: [("x", binding(x))], extraInputs: [("shape", ints(shape))],
            output: y)
    }

    public mutating func transpose(_ x: String, into y: Tensor, perm: [Int]) {
        emit(
            type: "transpose", inputs: [("x", binding(x))], extraInputs: [("perm", ints(perm))],
            output: y)
    }

    public mutating func gather(_ x: String, indices: String, into y: Tensor, axis: Int) {
        emit(
            type: "gather",
            inputs: [("x", binding(x)), ("indices", binding(indices))],
            extraInputs: [("axis", integer(axis))],
            output: y
        )
    }

    public mutating func scaledDotProductAttention(
        query: String, key: String, value: String, into y: Tensor, scale: Double
    ) {
        emit(
            type: "scaled_dot_product_attention",
            inputs: [("query", binding(query)), ("key", binding(key)), ("value", binding(value))],
            extraInputs: [("scale", scalar(scale))],
            output: y
        )
    }

    /// Keeps the last `size` elements of each axis, which is how a sliding window trims its cache.
    public mutating func sliceLast(_ x: String, into y: Tensor, begin: [Int], size: [Int]) {
        emit(
            type: "slice_by_size",
            inputs: [("x", binding(x))],
            extraInputs: [("begin", ints(begin)), ("size", ints(size))],
            output: y
        )
    }

    /// The whole model, ready for `protoc --encode=CoreML.Specification.Model`.
    public func render(specificationVersion: Int = 7) -> String {
        let featureInputs = inputs.map { tensor in
            """
                input {
                  name: "\(tensor.name)"
                  type {
                    multiArrayType { \(tensor.shape.map { "shape: \($0) " }.joined())dataType: FLOAT16 }
                  }
                }
            """
        }.joined(separator: "\n")
        let featureOutputs = outputs.map { tensor in
            """
                output {
                  name: "\(tensor.name)"
                  type {
                    multiArrayType { \(tensor.shape.map { "shape: \($0) " }.joined())dataType: FLOAT16 }
                  }
                }
            """
        }.joined(separator: "\n")
        let functionInputs = inputs.map { tensor in
            """
                  inputs {
                    name: "\(tensor.name)"
                    type { \(tensor.typeText) }
                  }
            """
        }.joined(separator: "\n")
        return """
            specificationVersion: \(specificationVersion)
            description {
            \(featureInputs)
            \(featureOutputs)
            }
            mlProgram {
              version: 1
              functions {
                key: "main"
                value {
            \(functionInputs)
                  opset: "CoreML7"
                  block_specializations {
                    key: "CoreML7"
                    value {
                      \(outputs.map { "outputs: \"\($0.name)\"" }.joined(separator: "\n                      "))
                      \(operations.joined(separator: "\n"))
                    }
                  }
                }
              }
            }

            """
    }

    private func binding(_ name: String) -> String { "arguments { name: \"\(name)\" }" }

    private func bool(_ value: Bool) -> String {
        """
        arguments {
                          value {
                            type { tensorType { dataType: BOOL } }
                            immediateValue { tensor { bools { values: \(value) } } }
                          }
                        }
        """
    }

    private func ints(_ values: [Int]) -> String {
        """
        arguments {
                          value {
                            type { tensorType { dataType: INT32 rank: 1 dimensions { constant { size: \(values.count) } } } }
                            immediateValue { tensor { ints { \(values.map { "values: \($0) " }.joined())} } }
                          }
                        }
        """
    }

    /// A fp16 scalar as raw little-endian bytes. The `floats` field carries four bytes per value,
    /// so a FLOAT16 immediate written there is rejected as having the wrong element count; the
    /// raw bytes form keeps the type and the storage in agreement.
    private func halfScalar(_ value: Double) -> String {
        let bits = Float16(Float(value)).bitPattern
        let escaped = String(format: "\\x%02x\\x%02x", bits & 0xff, bits >> 8)
        return """
            arguments {
                              value {
                                type { tensorType { dataType: FLOAT16 rank: 0 } }
                                immediateValue { tensor { bytes { values: "\(escaped)" } } }
                              }
                            }
            """
    }

    /// A fp16 one-element vector, for an op that will not broadcast a rank-0 constant.
    private func halfVector(_ value: Double) -> String {
        let bits = Float16(Float(value)).bitPattern
        let escaped = String(format: "\\x%02x\\x%02x", bits & 0xff, bits >> 8)
        return """
            arguments {
                              value {
                                type { tensorType { dataType: FLOAT16 rank: 1 dimensions { constant { size: 1 } } } }
                                immediateValue { tensor { bytes { values: "\(escaped)" } } }
                              }
                            }
            """
    }

    private func scalar(_ value: Double) -> String {
        """
        arguments {
                          value {
                            type { tensorType { dataType: FLOAT32 rank: 0 } }
                            immediateValue { tensor { floats { values: \(value) } } }
                          }
                        }
        """
    }

    private func integer(_ value: Int) -> String {
        """
        arguments {
                          value {
                            type { tensorType { dataType: INT32 } }
                            immediateValue { tensor { ints { values: \(value) } } }
                          }
                        }
        """
    }

    private mutating func emit(
        type: String, inputs pairs: [(String, String)], extraInputs: [(String, String)] = [],
        output: Tensor
    ) {
        let all = pairs + extraInputs
        let rendered = all.map { key, value in
            """
                            inputs {
                              key: "\(key)"
                              value { \(value) }
                            }
            """
        }.joined(separator: "\n")
        operations.append(
            """
                        operations {
                          type: "\(type)"
            \(rendered)
                          outputs {
                            name: "\(output.name)"
                            type { \(output.typeText) }
                          }
                        }
            """
        )
    }
}
