import Foundation
import Testing

@testable import PlamoCore

private actor Counter: CompletionProviding {
    private var calls = 0

    var count: Int { calls }

    func complete(_ request: CompletionRequest) async throws -> Completion {
        calls += 1
        return Completion(text: "translated \(calls)", tokens: 1, seconds: 0.001)
    }
}

@Test func aRepeatedRequestIsAnsweredFromMemory() async throws {
    let counter = Counter()
    let cache = CachingProvider(inner: counter)
    let request = CompletionRequest(prompt: "hello", maxTokens: 8, stop: [])
    let first = try await cache.complete(request)
    let second = try await cache.complete(request)
    #expect(first == second)
    #expect(await counter.count == 1)
    #expect(await cache.hits == 1)
    #expect(await cache.misses == 1)
}

@Test func theTokenCapAndStopListArePartOfTheKey() async throws {
    let counter = Counter()
    let cache = CachingProvider(inner: counter)
    _ = try await cache.complete(CompletionRequest(prompt: "hello", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "hello", maxTokens: 9, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "hello", maxTokens: 8, stop: ["x"]))
    #expect(await counter.count == 3)
}

@Test func theOldestEntryIsDroppedPastTheCapacity() async throws {
    let counter = Counter()
    let cache = CachingProvider(inner: counter, capacity: 2)
    _ = try await cache.complete(CompletionRequest(prompt: "a", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "b", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "c", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "a", maxTokens: 8, stop: []))
    #expect(await counter.count == 4)
    #expect(await cache.hits == 0)
}

@Test func aHitRefreshesTheEntrySoTheCapacityDropsTheOther() async throws {
    let counter = Counter()
    let cache = CachingProvider(inner: counter, capacity: 2)
    _ = try await cache.complete(CompletionRequest(prompt: "a", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "b", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "a", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "c", maxTokens: 8, stop: []))
    _ = try await cache.complete(CompletionRequest(prompt: "a", maxTokens: 8, stop: []))
    #expect(await counter.count == 3)
    #expect(await cache.hits == 2)
}
