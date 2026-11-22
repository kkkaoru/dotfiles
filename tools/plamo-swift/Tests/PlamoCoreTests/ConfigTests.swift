import Foundation
import Testing

@testable import PlamoCore

/// The tiny config as a mutable dictionary, so one field can be broken at a time.
private func mutableConfig() throws -> [String: Any] {
    let data = Data(TinyModel.configJSON.utf8)
    guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw PlamoError.invalidConfig(field: "test fixture")
    }
    return object
}

private func decode(_ object: [String: Any]) throws -> PlamoConfig {
    try JSONDecoder().decode(
        PlamoConfig.self, from: try JSONSerialization.data(withJSONObject: object))
}

@Test(arguments: [
    "hidden_size", "head_dim", "num_attention_heads", "num_hidden_layers",
    "num_key_value_heads", "intermediate_size", "vocab_size", "window_size",
    "sliding_window_pattern",
])
func validateRejectsANonPositiveField(_ field: String) throws {
    var object = try mutableConfig()
    object[field] = 0
    #expect(throws: PlamoError.self) { try decode(object).validate() }
}

@Test func validateRejectsAHeadsMismatch() throws {
    var object = try mutableConfig()
    object["num_key_value_heads"] = 3
    #expect(throws: PlamoError.self) { try decode(object).validate() }
}

@Test(arguments: ["full_attention", "sliding_attention"])
func validateRequiresBothRopeTables(_ missing: String) throws {
    var object = try mutableConfig()
    var rope = try #require(object["rope_parameters"] as? [String: Any])
    rope.removeValue(forKey: missing)
    object["rope_parameters"] = rope
    #expect(throws: PlamoError.self) { try decode(object).validate() }
}

@Test func loadsAndValidatesAConfigFile() throws {
    let directory = try Fixtures.temporaryDirectory("config-load")
    try Data(TinyModel.configJSON.utf8).write(to: directory.appendingPathComponent("config.json"))
    let config = try PlamoConfig.load(directory: directory)
    #expect(config.vocabSize == 300)
}

@Test func reportsAMissingConfigFile() throws {
    let directory = try Fixtures.temporaryDirectory("config-missing")
    #expect(throws: PlamoError.self) { try PlamoConfig.load(directory: directory) }
}
