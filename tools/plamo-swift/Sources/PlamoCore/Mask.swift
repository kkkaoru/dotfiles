import Foundation

/// The additive causal mask a prefill needs, with an optional sliding window. A single decode step
/// needs none: its one query sees the whole cache, so the caller skips this.
public enum Mask {
    /// Row-major [queries][keys] values: 0 where the query may attend, -infinity elsewhere.
    public static func causal(queries: Int, keys: Int, window: Int?) throws -> [Float] {
        guard queries > 0, keys >= queries else {
            throw PlamoError.invalidRequest(reason: "mask shape")
        }
        if let window, window <= 0 { throw PlamoError.invalidRequest(reason: "mask window") }
        var values = [Float](repeating: -Float.infinity, count: queries * keys)
        let offset = keys - queries
        for query in 0..<queries {
            let last = query + offset
            let first = window.map { max(0, last - $0 + 1) } ?? 0
            for key in first...last { values[query * keys + key] = 0 }
        }
        return values
    }
}
