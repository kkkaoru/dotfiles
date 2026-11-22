import Foundation
import Testing
@testable import PlamoCore

/// The blob is little-endian, as `scripts/dump-tokenizer.py` writes it: a reader that guesses the
/// other byte order rejects this fixture instead of failing later, on real weights.
@Test func loadsTheDumpedTokenizerBlob() throws {
    let tokenizer = try Fixtures.tokenizer()
    let ids = tokenizer.encode("Hello, world!")
    #expect(!ids.isEmpty)
    #expect(try tokenizer.decode(ids) == "Hello, world!")
}

@Test func rejectsATruncatedBlob() throws {
    let data = try Data(contentsOf: Fixtures.tokenizerBlob)
    let blob = try Fixtures.temporaryDirectory("tokenizer-truncated").appendingPathComponent("blob.bin")
    try data.prefix(data.count / 2).write(to: blob)
    #expect(throws: PlamoError.self) {
        try PlamoTokenizer(blob: blob, meta: Fixtures.tokenizerMeta)
    }
}
