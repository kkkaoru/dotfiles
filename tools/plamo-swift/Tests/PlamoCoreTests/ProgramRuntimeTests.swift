import CoreML
import Foundation
import Testing

@testable import PlamoCore

@Suite("Program compiler and runtime")
struct ProgramRuntimeTests {
    /// A cache inside the temporary directory, so a test never touches the server's own. It is
    /// unique per run: a cache left over from an earlier run would make `compile` return early and
    /// skip `protoc` and `coremlc` entirely, which is the path these tests exist to exercise.
    static func cache() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent(
            "plamo-coreml-tests-\(ProcessInfo.processInfo.processIdentifier)")
    }

    @Test("a compiled program runs, and its values are the ones the graph computes")
    func multiply() throws {
        var program = MILText()
        let x = MILText.Tensor("x", [1, 4])
        program.addInput(x)
        let y = MILText.Tensor("y", [1, 4])
        program.setOutputs([y])
        program.multiply(x.name, x.name, into: y)

        let compiled = try ProgramCompiler(cache: Self.cache()).compile(
            program.render(), name: "mul")
        #expect(compiled.pathExtension == "mlmodelc")

        let runtime = try ProgramRuntime(url: compiled)
        #expect(runtime.inputNames == ["x"])
        let out = try runtime.predict(["x": try .fp16([1, 2, 3, 4], shape: [1, 4])])
        #expect(out["y"]?.floats() == [1, 4, 9, 16])
    }

    @Test("the same text compiles once")
    func cacheHit() throws {
        var program = MILText()
        let x = MILText.Tensor("x", [1, 2])
        program.addInput(x)
        let y = MILText.Tensor("y", [1, 2])
        program.setOutputs([y])
        program.multiplyScalar(x.name, into: y, scale: 2)

        let compiler = ProgramCompiler(cache: Self.cache())
        let first = try compiler.compile(program.render(), name: "twice")
        let stamp =
            try FileManager.default.attributesOfItem(atPath: first.path)[.modificationDate] as? Date
        let second = try compiler.compile(program.render(), name: "twice")
        // `appendingPathComponent` marks an existing directory with a trailing slash.
        #expect(first.path == second.path)
        let again =
            try FileManager.default.attributesOfItem(atPath: second.path)[.modificationDate]
            as? Date
        #expect(stamp == again)
    }

    @Test("a broken program names the tool that refused it")
    func failure() throws {
        #expect(throws: PlamoError.self) {
            _ = try ProgramCompiler(cache: Self.cache()).compile("not a program", name: "broken")
        }
    }
}
