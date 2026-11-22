import Foundation
import Testing

@testable import PlamoCore

@Test func masksAPrefillTriangularly() throws {
    let values = try Mask.causal(queries: 2, keys: 2, window: nil)
    #expect(values == [0, -Float.infinity, 0, 0])
}

@Test func masksAWindowedPrefillToTheTail() throws {
    // Three queries over four keys with a window of two: each query sees itself and one before it.
    let values = try Mask.causal(queries: 3, keys: 4, window: 2)
    #expect(
        values == [
            0, 0, -Float.infinity, -Float.infinity,
            -Float.infinity, 0, 0, -Float.infinity,
            -Float.infinity, -Float.infinity, 0, 0,
        ])
}

@Test func aTrimmedCacheHoldsFewerKeysThanQueries() throws {
    // A sliding layer trims to the window, so a prompt longer than it has queries without keys. The
    // row must stay finite (all -infinity would be NaN after the softmax) and must not throw.
    let values = try Mask.causal(queries: 3, keys: 2, window: 2)
    #expect(values.count == 6)
    #expect(!values.allSatisfy { $0 == -Float.infinity })
    #expect(values[0] == 0)
    #expect(values[5] == 0)
}

@Test func rejectsAnEmptyMask() {
    #expect(throws: PlamoError.self) { try Mask.causal(queries: 0, keys: 1, window: nil) }
    #expect(throws: PlamoError.self) { try Mask.causal(queries: 1, keys: 0, window: nil) }
    #expect(throws: PlamoError.self) { try Mask.causal(queries: 1, keys: 1, window: 0) }
}

@Test func prefetchWarmsARealFile() throws {
    let directory = try Fixtures.temporaryDirectory("prefetch")
    let url = directory.appendingPathComponent(Prefetch.weightsName)
    let payload = Data(repeating: 7, count: 1 << 20)
    try payload.write(to: url)
    Prefetch.weights(in: directory)
    // Best effort: the advice either succeeded or was declined, and the file still reads back.
    #expect(try Data(contentsOf: url) == payload)
}

@Test func prefetchIgnoresAMissingFile() throws {
    let directory = try Fixtures.temporaryDirectory("prefetch-missing")
    Prefetch.weights(in: directory)
    Prefetch.warm(directory)
}

@Test func reportsThePromptTokensItCounted() async throws {
    let router = Router(
        provider: StubProvider(
            completion: Completion(text: "hello", promptTokens: 11, tokens: 3, seconds: 0.01)))
    let body = Data(#"{"prompt":"hi"}"#.utf8)
    let response = await router.respond(
        to: HTTPRequest(method: "POST", path: "/v1/completions", body: body))
    #expect(response.status == "200 OK")
    #expect(response.body.contains(#""prompt_tokens":11"#))
    #expect(response.body.contains(#""total_tokens":14"#))
}
