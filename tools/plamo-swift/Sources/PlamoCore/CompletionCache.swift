import Foundation

/// Greedy completions repeat constantly — a redrawn block, a scrolled screen, a resumed session — so
/// the server answers a repeat from memory instead of decoding it again. `scripts/plamo-mlx-server.py`
/// does the same on disk; this keeps it in RAM, where a hit costs a dictionary lookup.
///
/// Every request the router serves is greedy, so the prompt, the token cap and the stop list are the
/// whole key. The oldest entry is dropped past `capacity`, which bounds the memory a cache of short
/// translations can take.
public actor CachingProvider: CompletionProviding {
    private struct Key: Hashable {
        let prompt: String
        let maxTokens: Int
        let stop: [String]
    }

    private let inner: any CompletionProviding
    private let capacity: Int
    private var entries: [Key: Completion] = [:]
    private var order: [Key] = []
    private var hitCount = 0
    private var missCount = 0

    public init(inner: any CompletionProviding, capacity: Int = 512) {
        self.inner = inner
        self.capacity = max(capacity, 1)
    }

    public var hits: Int { hitCount }
    public var misses: Int { missCount }

    public func complete(_ request: CompletionRequest) async throws -> Completion {
        let key = Key(prompt: request.prompt, maxTokens: request.maxTokens, stop: request.stop)
        if let cached = entries[key] {
            hitCount += 1
            touch(key)
            return cached
        }
        missCount += 1
        let completion = try await inner.complete(request)
        entries[key] = completion
        order.append(key)
        while order.count > capacity {
            entries.removeValue(forKey: order.removeFirst())
        }
        return completion
    }

    private func touch(_ key: Key) {
        if let index = order.firstIndex(of: key) {
            order.remove(at: index)
            order.append(key)
        }
    }
}
