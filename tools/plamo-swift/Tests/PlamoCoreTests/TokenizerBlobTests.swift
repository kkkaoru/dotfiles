import Foundation
import Testing

@testable import PlamoCore

/// Writes the little-endian layout `scripts/dump-tokenizer.py` produces, one field at a time, so a
/// single malformed field can be handed to the loader.
private func littleEndian(_ value: Int32) -> Data {
    withUnsafeBytes(of: value.littleEndian) { Data($0) }
}

private func littleEndian(_ value: Int64) -> Data {
    withUnsafeBytes(of: value.littleEndian) { Data($0) }
}

private func blob(
    rowCount: Int32 = 0, pairCount: Int32 = 0, byteTokens: Int = 256, tokens: [Data] = [],
    magic: String = "PLT1"
) -> Data {
    var data = Data(magic.utf8)
    data.append(littleEndian(rowCount))
    for _ in 0..<Int(rowCount * 4) { data.append(littleEndian(Int32(0))) }
    data.append(littleEndian(pairCount))
    for _ in 0..<Int(pairCount) {
        data.append(littleEndian(Int64(0)))
        data.append(littleEndian(Int32(0)))
    }
    for _ in 0..<byteTokens { data.append(littleEndian(Int32(0))) }
    data.append(littleEndian(Int32(tokens.count)))
    for token in tokens {
        data.append(littleEndian(Int32(token.count)))
        data.append(token)
    }
    return data
}

private func meta(boundary: String = "\u{ee00}") -> Data {
    Data(
        """
        {"boundary_char":"\(boundary)","boundary_token_id":1000,"unknown_score":-1,
         "invalid_score":-2,"special_pattern":"(<x>)","break_pattern":"zzz"}
        """.utf8)
}

private func load(_ blob: Data, meta metaData: Data? = nil) throws -> PlamoTokenizer {
    let directory = try Fixtures.temporaryDirectory("blob")
    let blobURL = directory.appendingPathComponent("blob.bin")
    let metaURL = directory.appendingPathComponent("blob.json")
    try blob.write(to: blobURL)
    try (metaData ?? meta()).write(to: metaURL)
    return try PlamoTokenizer(blob: blobURL, meta: metaURL)
}

@Test func aMinimalBlobDecodesItsTokens() throws {
    let tokenizer = try load(blob(tokens: [Data("a".utf8)]))
    let text = try tokenizer.decode([0])
    #expect(text == "a")
}

@Test func rejectsABadMagic() {
    #expect(throws: PlamoError.self) { try load(blob(magic: "PLT0")) }
}

@Test func rejectsATokenThatIsNotUTF8() {
    #expect(throws: PlamoError.self) { try load(blob(tokens: [Data([0xFF])])) }
}

@Test func rejectsAnEmptyBoundaryCharacter() {
    #expect(throws: PlamoError.self) {
        try load(blob(tokens: [Data("a".utf8)]), meta: meta(boundary: ""))
    }
}

@Test func decodeRejectsAnUnknownTokenID() throws {
    let tokenizer = try load(blob(tokens: [Data("a".utf8)]))
    #expect(throws: PlamoError.self) { try tokenizer.decode([7]) }
}

@Test func decodeRejectsBytesThatAreNotUTF8() throws {
    // A single 0xFF byte token is a legal token but not a legal string.
    let tokenizer = try load(blob(tokens: [Data("<0xFF>".utf8)]))
    #expect(throws: PlamoError.self) { try tokenizer.decode([0]) }
}

@Test func decodeDropsTheBoundaryToken() throws {
    let tokenizer = try load(blob(tokens: [Data("a".utf8)]))
    let text = try tokenizer.decode([1_000, 0])
    #expect(text == "a")
}

@Test func promptIDsAddsBOSOnce() throws {
    let tokenizer = try load(blob(tokens: [Data("a".utf8)]))
    let ids = tokenizer.promptIDs("a", bos: 5)
    #expect(ids.first == 5)
    #expect(tokenizer.promptIDs("a", bos: ids.first!) == ids)
}
