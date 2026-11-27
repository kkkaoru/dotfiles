import CoreML
import Foundation
import MLX
import Testing

@testable import PlamoCore

/// The whole decoder against the real checkpoint: off unless `PLAMO_DECODER` names a model
/// directory, since it loads about a gigabyte and compiles two models.
@Suite("decoder", .serialized)
struct DecoderBench {
    /// The checkpoint the service serves, used when the environment does not name another.
    static let defaultDirectory =
        FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit")

    static var directory: URL? {
        let path = ProcessInfo.processInfo.environment["PLAMO_DECODER"] ?? ""
        guard !path.isEmpty else { return nil }
        let url = URL(fileURLWithPath: path)
        return FileManager.default.fileExists(atPath: url.path) ? url : nil
    }

    @Test("the head program renders the final norm and the lm_head matmul")
    func headProgram() throws {
        let config = try PlamoConfig.load(directory: Self.defaultDirectory)
        let text = CoreMLDecoder.headProgram(config: config).render()
        #expect(text.contains("name: \"w_final_norm\""))
        #expect(text.contains("name: \"w_lm_head\""))
        #expect(text.contains("type: \"rsqrt\""))
        #expect(text.contains("type: \"matmul\""))
        #expect(text.contains("outputs: \"logits\""))
    }

    @Test("a layer takes the table its sliding pattern selects")
    func ropeParameters() throws {
        let config = try PlamoConfig.load(directory: Self.defaultDirectory)
        let sliding = try CoreMLDecoder.ropeParameters(config: config, index: 0)
        // `sliding_window_pattern: 8` makes every eighth layer full attention: 7 and 15, not 3.
        let full = try CoreMLDecoder.ropeParameters(config: config, index: 7)
        #expect(sliding.ropeTheta == config.ropeParameters["sliding_attention"]?.ropeTheta)
        #expect(full.ropeTheta == config.ropeParameters["full_attention"]?.ropeTheta)
    }

    @Test("the whole decoder produces a token from the real checkpoint")
    func decode() throws {
        guard let directory = Self.directory else { return }
        var decoder = try CoreMLDecoder(directory: directory)
        let bos = 1
        let first = try decoder.nextToken(after: bos)
        #expect(first >= 0 && first < 1_000_000)
        let second = try decoder.nextToken(after: first)
        #expect(second >= 0 && second < 1_000_000)
        FileHandle.standardError.write(
            Data("decoder tokens \(bos) -> \(first) -> \(second)\n".utf8))
    }
}
