import Foundation
import MLX
import Testing

@testable import PlamoCore

/// The Core ML decoder against `TinyModel`: a complete two-layer checkpoint small enough to build
/// and compile in a test, so the decode path is covered without the gigabyte real checkpoint.
@Suite("coreml decoder", .serialized)
struct CoreMLDecoderTests {
    /// A decoded directory plus the arrays it was written from.
    private static func tiny() throws -> (URL, [String: MLXArray]) {
        let directory = try Fixtures.temporaryDirectory("coreml-decoder")
        try TinyModel.write(to: directory)
        return (directory, try TinyModel.arrays())
    }

    @Test("the decoder runs a whole step on a two-layer checkpoint")
    func decodes() throws {
        let (directory, _) = try Self.tiny()
        let config = try TinyModel.config()
        var decoder = try CoreMLDecoder(directory: directory)
        let logits = try decoder.logits(forToken: Int(config.bosTokenId))
        #expect(logits.count == config.vocabSize)
        let first = try decoder.nextToken(after: Int(config.bosTokenId))
        #expect(first >= 0 && first < config.vocabSize)
    }

    @Test("reset drops the cache so the next token starts a new sequence")
    func resetStartsOver() throws {
        let (directory, _) = try Self.tiny()
        let config = try TinyModel.config()
        var decoder = try CoreMLDecoder(directory: directory)
        let bos = Int(config.bosTokenId)
        let first = try decoder.nextToken(after: bos)
        decoder.reset()
        let again = try decoder.nextToken(after: bos)
        #expect(again == first)
    }

    @Test("a token outside the vocabulary is rejected")
    func rejectsATokenOutsideTheVocabulary() throws {
        let (directory, _) = try Self.tiny()
        let config = try TinyModel.config()
        var decoder = try CoreMLDecoder(directory: directory)
        #expect(throws: PlamoError.self) {
            _ = try decoder.logits(forToken: config.vocabSize + 1)
        }
    }

    @Test("a checkpoint without the requested tensor is reported by name")
    func reportsAMissingTensor() throws {
        let (_, arrays) = try Self.tiny()
        #expect(throws: PlamoError.self) {
            _ = try CoreMLDecoder.tensor("model.nope.weight", shape: [1], arrays: arrays)
        }
    }

    @Test("a tensor whose bytes do not fill the declared shape is rejected")
    func rejectsAMismatchedTensorSize() throws {
        let value = MLXArray([Float](repeating: 0, count: 4))
        #expect(throws: PlamoError.self) {
            _ = try CoreMLDecoder.half(value, shape: [3, 3])
        }
    }

    @Test("a layer takes the table its sliding pattern selects, and a full layer the other")
    func ropeParametersPerLayer() throws {
        let config = try TinyModel.config()
        // `sliding_window_pattern: 2` makes every second layer full attention: index 1, not 0.
        let sliding = try CoreMLDecoder.ropeParameters(config: config, index: 0)
        let full = try CoreMLDecoder.ropeParameters(config: config, index: 1)
        #expect(sliding.ropeTheta == config.ropeParameters["sliding_attention"]?.ropeTheta)
        #expect(full.ropeTheta == config.ropeParameters["full_attention"]?.ropeTheta)
    }

    @Test("the head program flattens the layer output before the norm and the matmul")
    func headProgramFlattens() throws {
        let text = CoreMLDecoder.headProgram(config: try TinyModel.config()).render()
        #expect(text.contains("h_flat"))
        #expect(text.contains("reshape"))
        #expect(text.contains("w_lm_head"))
    }
}
