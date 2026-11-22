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
