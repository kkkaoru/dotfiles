import Foundation
import Testing

@testable import PlamoCore

@Suite("MIL text emission")
struct MILTextTests {
    /// The spike that proved the pipeline: one fp16 matmul, compiled by coremlc and run on
    /// CPU_AND_NE. The rendered text must keep the three things the validator insisted on.
    @Test("a matmul program carries the features, the version and the bool inputs")
    func matmulProgram() {
        var program = MILText()
        let x = MILText.Tensor("x", [1, 1536])
        let w = MILText.Tensor("w", [1536, 1536])
        let y = MILText.Tensor("y", [1, 1536])
        program.addInput(x)
        program.addInput(w)
        program.matmul("x", "w", into: y)
        program.setOutputs(["y"])
        let text = program.render()

        #expect(text.hasPrefix("specificationVersion: 7\n"))
        #expect(text.contains("multiArrayType { shape: 1 shape: 1536 dataType: FLOAT16 }"))
        #expect(text.contains("tensorType { dataType: FLOAT16 rank: 2 dimensions { constant { size: 1536 } }"))
        #expect(text.contains("key: \"transpose_y\""))
        #expect(text.contains("bools { values: false }"))
        #expect(text.contains("name: \"y\""))
        // A matmul with the transposes as attributes is rejected; they must stay inputs.
        #expect(!text.contains("attributes {"))
    }

    @Test("attention arithmetic renders every operation it is asked for")
    func layerArithmetic() {
        var program = MILText()
        program.addInput(MILText.Tensor("h", [1, 1536]))
        program.addInput(MILText.Tensor("k", [1, 1536]))
        program.addInput(MILText.Tensor("v", [1, 1536]))
        let scores = MILText.Tensor("scores", [1, 1])
        program.matmul("h", "k", into: scores)
        let weights = MILText.Tensor("weights", [1, 1])
        program.softmax("scores", into: weights, axis: -1)
        let summed = MILText.Tensor("summed", [1, 1536])
        program.matmul("weights", "v", into: summed)
        let joined = MILText.Tensor("joined", [1, 3072])
        program.concat(["h", "k"], into: joined, axis: -1)
        let total = MILText.Tensor("total", [1, 3072])
        program.add("joined", "joined", into: total)
        let scaled = MILText.Tensor("scaled", [1, 3072])
        program.multiply("total", "total", into: scaled)
        program.setOutputs(["scaled"])
        let text = program.render()

        for op in ["matmul", "softmax", "concat", "add", "mul"] {
            #expect(text.contains("type: \"\(op)\""))
        }
        #expect(text.contains("key: \"axis\""))
        #expect(text.contains("ints { values: -1 }"))
        #expect(text.contains("[arguments { name: \"h\" }, arguments { name: \"k\" }]"))
    }
}
