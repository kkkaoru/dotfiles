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
    private var outputs: [String] = []

    public init() {}

    public mutating func addInput(_ tensor: Tensor) {
        inputs.append(tensor)
    }

    public mutating func setOutputs(_ names: [String]) {
        outputs = names
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
            inputs: [("values", "[\(tensors.map(binding).joined(separator: ", "))]")],
            extraInputs: [("axis", integer(axis)), ("interleave", bool(false))],
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
        let featureOutputs = outputs.compactMap { name in
            inputs.first { $0.name == name }
        }.map { tensor in
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
                  opset: "CoreML5"
                  block_specializations {
                    key: "CoreML5"
                    value {
                      outputs: "\(outputs.first ?? "")"
                      operations {
            \(operations.joined(separator: "\n"))
                      }
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
                        {
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
