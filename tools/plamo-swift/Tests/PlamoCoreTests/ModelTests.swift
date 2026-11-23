import Foundation
import MLX
import Testing

@testable import PlamoCore

private func tinyDirectory(_ name: String) throws -> URL {
    let directory = try Fixtures.temporaryDirectory(name)
    try TinyModel.write(to: directory)
    return directory
}

@Test func loadsATinyModelAndReportsItsShape() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-load"))
    #expect(model.config.numHiddenLayers == 2)
    #expect(model.config.windowSize == 3)
    #expect(model.position == 0)
}

@Test func aPrefillReturnsOneLogitRowPerVocabularyEntry() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-prefill"))
    let logits = try model.logits(ids: [2, 5, 7])
    MLX.eval(logits)
    // One row for the single last position: the matmul keeps the batch axis.
    #expect(logits.shape == [1, 1, model.config.vocabSize])
    #expect(model.position == 3)
    // The cache is reset for the next prompt, so positions start over instead of accumulating.
    model.resetCache()
    #expect(model.position == 0)
}

@Test func aPrefillLongerThanTheWindowMixesBothMaskKinds() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-window"))
    // The window is 3 and every second layer is full attention, so this pass needs the trimmed
    // sliding mask and the full one, and must not throw on the shorter key count.
    let logits = try model.logits(ids: [2, 3, 4, 5, 6])
    MLX.eval(logits)
    #expect(logits.shape == [1, 1, model.config.vocabSize])
    #expect(model.position == 5)
}

@Test func generateStopsAtTheEndToken() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-generate"))
    let produced = try model.generate(
        prompt: [2, 3], maxTokens: 4, stopIDs: Set(model.config.eosTokenId))
    #expect(produced.count <= 4)
    #expect(!produced.contains(where: { model.config.eosTokenId.contains($0) }))
}

@Test func steppingPastTheWindowMatchesOnePrefill() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-cache-trim"))
    // Five tokens against a window of three: the cache drops its first two keys, and a key keeps
    // the rotation it was written with, so stepping through the tokens must land exactly where one
    // pass over all five lands.
    let whole = try model.logits(ids: [2, 3, 4, 5, 6])
    MLX.eval(whole)
    model.resetCache()
    var logits = try model.logits(ids: [2, 3, 4])
    MLX.eval(logits)
    for id in [Int32(5), 6] {
        logits = try model.logits(token: MLXArray([id]))
        MLX.eval(logits)
    }
    // The two paths reduce in a different order, so the float32 logits differ by ~0.009; the trim
    // that ran before this fix moved them by 4.05, which is still 80x outside this tolerance.
    #expect(allClose(whole, logits, rtol: 1e-2, atol: 1e-2).item(Bool.self))
    #expect(
        whole.argMax(axis: -1).item(Int32.self) == logits.argMax(axis: -1).item(Int32.self))
}

@Test func generateDropsTheRestOfAChunkAtAStopToken() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-generate-stop"))
    // A chunk carries two steps, so the second is already built when the first turns out to be a
    // stop token: the reply must end before it, not include it.
    let first = try model.logits(ids: [2, 3]).argMax(axis: -1).item(Int32.self)
    model.resetCache()
    let produced = try model.generate(prompt: [2, 3], maxTokens: 4, stopIDs: [first])
    #expect(produced.isEmpty)
}

@Test func aTokenArrayProducesTheSameLogitsAsTheIDsForm() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-decode"))
    let fromIDs = try model.logits(ids: [5])
    MLX.eval(fromIDs)
    model.resetCache()
    let fromArray = try model.logits(token: MLXArray([Int32(5)]))
    MLX.eval(fromArray)
    #expect(fromIDs.shape == fromArray.shape)
    #expect(allClose(fromIDs, fromArray).item(Bool.self))
}

@Test func reportsAnEmptyPromptAndAMissingTensor() throws {
    let model = try PlamoModel.load(directory: try tinyDirectory("model-errors"))
    #expect(throws: PlamoError.self) { try model.logits(ids: []) }
    #expect(throws: PlamoError.self) {
        try PlamoModel.make(config: try TinyModel.config(), arrays: [:])
    }
}

@Test func reportsAMissingWeightsFile() throws {
    let directory = try Fixtures.temporaryDirectory("model-missing")
    try Data(TinyModel.configJSON.utf8).write(to: directory.appendingPathComponent("config.json"))
    #expect(throws: PlamoError.self) { try PlamoModel.load(directory: directory) }
}
