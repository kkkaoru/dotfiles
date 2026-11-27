import Foundation

/// Emits a Core ML ML Program as protobuf **text**, which `protoc --encode` turns into a
/// `model.mlmodel` and `coremlc` compiles. This exists because the only usual way to author an ML
/// Program is coremltools, which is Python: here the graph is written by Swift and the toolchain is
/// a C++ encoder plus Xcode's compiler.
///
/// The text is deliberately the wire schema, not a DSL: `Model.mlProgram` is `MILSpec.Program`, so
/// every value carries its `tensorType` and every `matmul` takes `transpose_x`/`transpose_y` as
/// `BOOL` inputs (attributes are rejected by the validator). `specificationVersion` must be at
/// least 7 for `FLOAT16` multi-array features.
public struct MILText {
    /// One named tensor: the decoder's values are all fp16 tensors of a known rank.
    public struct Tensor: Equatable {
        public let name: String
        public let shape: [Int]
        public init(_ name: String, _ shape: [Int]) {
            self.name = name
            self.shape = shape
        }

        internal var typeText: String {
            let dimensions = shape.map { "dimensions { constant { size: \($0) } }" }.joined(
                separator: " ")
            return "tensorType { dataType: \(MILText.dataType) rank: \(shape.count) \(dimensions) }"
        }
    }

    /// A model input, a constant or an operation output.
    public private(set) var inputs: [Tensor] = []
    /// Comment lines carried into the rendered model, so the generated text names what it binds.
    public private(set) var annotations: [String] = []
    private var operations: [String] = []
    private var opOutputs: [Tensor] = []
    private var outputs: [Tensor] = []

    /// The dtype every tensor in every program is written with. `PLAMO_DTYPE=fp32` switches the
    /// whole graph, which is how the BNNS fp16 crash is being narrowed down.
    public static let dataType: String =
        ProcessInfo.processInfo.environment["PLAMO_DTYPE"] == "fp32" ? "FLOAT32" : "FLOAT16"

    /// An empty program: no inputs, no operations, no outputs.
    public init() {}

    /// Binds one more model input, in the order the model declares them.
    public mutating func addInput(_ tensor: Tensor) {
        inputs.append(tensor)
    }

    /// The tensors the model returns.
    public mutating func setOutputs(_ tensors: [Tensor]) {
        outputs = tensors
    }

    /// Adds a comment line to the rendered model, so the text names what it binds.
    public mutating func annotate(_ line: String) {
        annotations.append(line)
    }

    /// How many operations the program has, for bisecting a plan-builder crash.
    public var opCount: Int { operations.count }

    /// Debug aid: keep only the first `count` operations, ending the program at the last one.
    public mutating func truncate(ops count: Int) {
        guard count < operations.count else { return }
        operations = Array(operations.prefix(count))
        outputs = [opOutputs[count - 1]]
    }

    /// `y = x @ w`, with both transposes spelled out as the validator requires.
    ///
    /// `transposeY` is how a checkpoint's `[out, in]` weight is multiplied as `x @ w`: transposing
    /// it in the graph costs nothing, while materializing a transposed copy in Swift took 11 s per
    /// weight, because MLX has to gather every element instead of copying bytes.
    public mutating func matmul(
        _ x: String, _ w: String, into y: Tensor, transposeY: Bool = false
    ) {
        emit(
            type: "matmul",
            inputs: [("x", binding(x)), ("y", binding(w))],
            extraInputs: [("transpose_x", bool(false)), ("transpose_y", bool(transposeY))],
            output: y
        )
    }

    /// `y = a + b`.
    public mutating func add(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "add", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    /// `y = a * b`.
    public mutating func multiply(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "mul", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    /// `y = softmax(x)` along `axis`.
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
            inputs: [
                ("values", tensors.map { "arguments { name: \"\($0)\" }" }.joined(separator: " "))
            ],
            extraInputs: [("axis", integer(axis)), ("interleave", bool(false))],
            output: y
        )
    }

    /// `y = a - b`.
    public mutating func subtract(_ a: String, _ b: String, into y: Tensor) {
        emit(type: "sub", inputs: [("x", binding(a)), ("y", binding(b))], output: y)
    }

    /// `y = a / b`.
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

    /// `y = 1 / sqrt(x + epsilon)`.
    public mutating func rsqrt(_ x: String, into y: Tensor, epsilon: Double) {
        emit(
            type: "rsqrt", inputs: [("x", binding(x)), ("epsilon", halfScalar(epsilon))], output: y)
    }

    /// `y = sigmoid(x)`.
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

    /// `y = x` viewed as `shape`, with no data movement.
    public mutating func reshape(_ x: String, into y: Tensor, shape: [Int]) {
        emit(
            type: "reshape", inputs: [("x", binding(x))], extraInputs: [("shape", ints(shape))],
            output: y)
    }

    /// `y = x` with its axes permuted by `perm`.
    public mutating func transpose(_ x: String, into y: Tensor, perm: [Int]) {
        emit(
            type: "transpose", inputs: [("x", binding(x))], extraInputs: [("perm", ints(perm))],
            output: y)
    }

    /// `y = x[indices]` along `axis`.
    public mutating func gather(_ x: String, indices: String, into y: Tensor, axis: Int) {
        emit(
            type: "gather",
            inputs: [("x", binding(x)), ("indices", binding(indices))],
            extraInputs: [("axis", integer(axis))],
            output: y
        )
    }

    /// The fused attention op: `y = softmax(q @ kᵀ * scale) @ v`.
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
            let shapeText = tensor.shape.map { "shape: \($0) " }.joined()
            return """
                    input {
                      name: "\(tensor.name)"
                      type {
                        multiArrayType { \(shapeText)dataType: FLOAT16 }
                      }
                    }
                """
        }.joined(separator: "\n")
        let featureOutputs = outputs.map { tensor in
            let shapeText = tensor.shape.map { "shape: \($0) " }.joined()
            return """
                    output {
                      name: "\(tensor.name)"
                      type {
                        multiArrayType { \(shapeText)dataType: FLOAT16 }
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
        let outputLines = outputs.map { "outputs: \"\($0.name)\"" }
            .joined(separator: "\n                      ")
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
            \(annotations.map { "# \($0)" }.joined(separator: "\n            "))
                      \(outputLines)
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
        let shape = "rank: 1 dimensions { constant { size: \(values.count) } }"
        let list = values.map { "values: \($0) " }.joined()
        return """
            arguments {
                              value {
                                type { tensorType { dataType: INT32 \(shape) } }
                                immediateValue { tensor { ints { \(list)} } }
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
        let shape = "rank: 1 dimensions { constant { size: 1 } }"
        return """
            arguments {
                              value {
                                type { tensorType { dataType: FLOAT16 \(shape) } }
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
        opOutputs.append(output)
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
