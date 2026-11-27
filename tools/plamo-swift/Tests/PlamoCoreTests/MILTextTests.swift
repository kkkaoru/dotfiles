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
        let x = MILText.Tensor("x", [1, 1_536])
        let w = MILText.Tensor("w", [1_536, 1_536])
        let y = MILText.Tensor("y", [1, 1_536])
        program.addInput(x)
        program.addInput(w)
        program.matmul("x", "w", into: y)
        program.setOutputs([y])
        let text = program.render()

        #expect(text.hasPrefix("specificationVersion: 7\n"))
        #expect(text.contains("multiArrayType { shape: 1 shape: 1536 dataType: FLOAT16 }"))
        #expect(
            text.contains(
                "tensorType { dataType: FLOAT16 rank: 2 dimensions { constant { size: 1536 } }"))
        #expect(text.contains("key: \"transpose_y\""))
        #expect(text.contains("bools { values: false }"))
        #expect(text.contains("name: \"y\""))
        // A matmul with the transposes as attributes is rejected; they must stay inputs.
        #expect(!text.contains("attributes {"))
    }

    @Test("attention arithmetic renders every operation it is asked for")
    func layerArithmetic() {
        var program = MILText()
        program.addInput(MILText.Tensor("h", [1, 1_536]))
        program.addInput(MILText.Tensor("k", [1, 1_536]))
        program.addInput(MILText.Tensor("v", [1, 1_536]))
        let scores = MILText.Tensor("scores", [1, 1])
        program.matmul("h", "k", into: scores)
        let weights = MILText.Tensor("weights", [1, 1])
        program.softmax("scores", into: weights, axis: -1)
        let summed = MILText.Tensor("summed", [1, 1_536])
        program.matmul("weights", "v", into: summed)
        let joined = MILText.Tensor("joined", [1, 3_072])
        program.concat(["h", "k"], into: joined, axis: -1)
        let total = MILText.Tensor("total", [1, 3_072])
        program.add("joined", "joined", into: total)
        let scaled = MILText.Tensor("scaled", [1, 3_072])
        program.multiply("total", "total", into: scaled)
        program.setOutputs([scaled])
        let text = program.render()

        for op in ["matmul", "softmax", "concat", "add", "mul"] {
            #expect(text.contains("type: \"\(op)\""))
        }
        #expect(text.contains("key: \"axis\""))
        #expect(text.contains("ints { values: -1 }"))
        #expect(text.contains("arguments { name: \"h\" } arguments { name: \"k\" }"))
    }

    @Test("the layer primitives emit the ops an RMSNorm and a SwiGLU need")
    func layerPrimitives() {
        var program = MILText()
        program.addInput(MILText.Tensor("h", [1, 1_536]))
        program.addInput(MILText.Tensor("w", [1_536, 1_536]))
        let mean = MILText.Tensor("mean", [1, 1])
        program.reduceMean("h", into: mean, axes: [-1], keepDims: true)
        let inv = MILText.Tensor("inv", [1, 1])
        program.rsqrt("mean", into: inv, epsilon: 1e-06)
        let gate = MILText.Tensor("gate", [1, 1_536])
        program.matmul("h", "w", into: gate)
        let activated = MILText.Tensor("activated", [1, 1_536])
        program.sigmoid("gate", into: activated)
        let gated = MILText.Tensor("gated", [1, 1_536])
        program.multiply("gate", "activated", into: gated)
        let shifted = MILText.Tensor("shifted", [1, 1_536])
        program.subtract("gated", "h", into: shifted)
        let scaled = MILText.Tensor("scaled", [1, 1_536])
        program.divide("shifted", "inv", into: scaled)
        let shaped = MILText.Tensor("shaped", [1, 1_536])
        program.reshape("scaled", into: shaped, shape: [1, 1_536])
        let moved = MILText.Tensor("moved", [1_536, 1])
        program.transpose("shaped", into: moved, perm: [1, 0])
        let picked = MILText.Tensor("picked", [1, 1_536])
        program.gather("h", indices: "h", into: picked, axis: 0)
        program.setOutputs([picked])
        let text = program.render()

        for op in [
            "reduce_mean", "rsqrt", "sigmoid", "real_div", "sub", "reshape", "transpose", "gather",
        ] {
            #expect(text.contains("type: \"\(op)\""))
        }
        #expect(text.contains("ints { values: -1 }"))
        #expect(text.contains("ints { values: 1 values: 0 }"))
        #expect(text.contains("dataType: INT32 rank: 1 dimensions { constant { size: 2 } }"))
        #expect(text.contains("key: \"keep_dims\""))
    }

    @Test("the scalar, sliced and attention ops render, and truncation keeps a prefix")
    func remainingOps() {
        var program = MILText()
        let x = MILText.Tensor("x", [1, 4])
        program.addInput(x)
        program.annotate("x = model.embed_tokens.weight")

        let added = MILText.Tensor("added", [1, 4])
        program.addScalar(x.name, into: added, epsilon: 1e-6)
        let scaled = MILText.Tensor("scaled", [1, 4])
        program.multiplyScalar(added.name, into: scaled, scale: 0.25)
        let sliced = MILText.Tensor("sliced", [1, 2])
        program.sliceLast(scaled.name, into: sliced, begin: [0, 2], size: [1, 2])
        let attended = MILText.Tensor("attended", [1, 4])
        program.scaledDotProductAttention(
            query: x.name, key: x.name, value: x.name, into: attended, scale: 0.5)
        program.setOutputs([attended])

        #expect(program.opCount == 4)
        let text = program.render()
        #expect(text.contains("# x = model.embed_tokens.weight"))
        #expect(text.contains("type: \"slice_by_size\""))
        #expect(text.contains("type: \"scaled_dot_product_attention\""))
        // The attention scale is a FLOAT32 rank-0 immediate, not a tensor input.
        #expect(text.contains("floats { values: 0.5 }"))

        program.truncate(ops: 2)
        #expect(program.opCount == 2)
        #expect(program.render().contains("outputs: \"scaled\""))
        // Asking for more ops than the program has keeps it whole.
        program.truncate(ops: 4)
        #expect(program.opCount == 2)
    }
}
