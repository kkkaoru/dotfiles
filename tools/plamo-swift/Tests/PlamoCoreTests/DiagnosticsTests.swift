import Foundation
import Testing

@testable import PlamoCore

@Suite("diagnostics")
struct DiagnosticsTests {
    private func temporaryStore() -> DiagnosticsStore {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("plamo-diagnostics-\(UUID().uuidString)/diagnostics.json")
        return DiagnosticsStore(file: file)
    }

    private let machine = ExecutionEnvironment(
        model: "Mac15,6", os: "24G90", chip: "Apple M3 Pro", gpu: "Apple M3 Pro",
        neuralEngine: true)
    private let otherMachine = ExecutionEnvironment(
        model: "Mac15,6", os: "24G90", chip: "Apple M3 Pro", gpu: "Apple M3 Pro",
        neuralEngine: false)

    @Test("fingerprint changes with every field")
    func fingerprint() {
        let base = machine.fingerprint
        #expect(base == machine.fingerprint)
        #expect(base != otherMachine.fingerprint)
        let differentOS = ExecutionEnvironment(
            model: "Mac15,6", os: "24G91", chip: "Apple M3 Pro", gpu: "Apple M3 Pro",
            neuralEngine: true)
        #expect(base != differentOS.fingerprint)
        let differentGPU = ExecutionEnvironment(
            model: "Mac15,6", os: "24G90", chip: "Apple M3 Pro", gpu: "Apple M4",
            neuralEngine: true)
        #expect(base != differentGPU.fingerprint)
    }

    @Test("the probe reports this machine")
    func probe() {
        let current = EnvironmentProbe.current()
        #expect(!current.model.isEmpty)
        #expect(!current.os.isEmpty)
        #expect(!current.chip.isEmpty)
        #expect(!current.gpu.isEmpty)
        #expect(current.fingerprint.contains(current.model))
    }

    @Test("a missing file is an empty cache")
    func missingFile() {
        let store = temporaryStore()
        #expect(store.load().entries.isEmpty)
        #expect(store.measurement(for: "coreml", environment: machine) == nil)
        #expect(store.fastest(of: ["coreml"], environment: machine) == nil)
    }

    @Test("a corrupt file is an empty cache")
    func corruptFile() throws {
        let store = temporaryStore()
        try FileManager.default.createDirectory(
            at: store.file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("not json".utf8).write(to: store.file)
        #expect(store.load().entries.isEmpty)
    }

    @Test("a measurement is kept under its own environment only")
    func recordPerEnvironment() throws {
        let store = temporaryStore()
        try store.record(
            EngineMeasurement(engine: "coreml", millisecondsPerStep: 12.5), environment: machine)

        let stored = try #require(store.measurement(for: "coreml", environment: machine))
        #expect(stored.millisecondsPerStep == 12.5)
        #expect(store.measurement(for: "coreml", environment: otherMachine) == nil)
        // The other environment's own number is not shadowed by the first one.
        try store.record(
            EngineMeasurement(engine: "mps", millisecondsPerStep: 20), environment: otherMachine)
        #expect(store.measurement(for: "mps", environment: machine) == nil)
        #expect(store.load().entries.count == 2)
    }

    @Test("a second measurement replaces the first for the same engine")
    func recordReplaces() throws {
        let store = temporaryStore()
        try store.record(
            EngineMeasurement(engine: "coreml", millisecondsPerStep: 30), environment: machine)
        try store.record(
            EngineMeasurement(engine: "coreml", millisecondsPerStep: 10), environment: machine)
        #expect(store.measurement(for: "coreml", environment: machine)?.millisecondsPerStep == 10)
        #expect(store.load().entries[machine.fingerprint]?.measurements.count == 1)
    }

    @Test("the fastest measured engine wins, unmeasured ones are ignored")
    func fastest() throws {
        let store = temporaryStore()
        try store.record(
            EngineMeasurement(engine: "coreml", millisecondsPerStep: 18), environment: machine)
        try store.record(
            EngineMeasurement(engine: "mps", millisecondsPerStep: 9), environment: machine)
        #expect(store.fastest(of: ["coreml", "mps", "mlx"], environment: machine)?.engine == "mps")
        #expect(store.fastest(of: ["mlx"], environment: machine) == nil)
    }

    @Test("the default file sits in the user's caches")
    func defaultFile() {
        #expect(DiagnosticsStore.defaultFile.lastPathComponent == "diagnostics.json")
        #expect(DiagnosticsStore.defaultFile.path.contains("plamo-swift"))
    }
}
