import Foundation

/// The additive causal mask a prefill needs, with an optional sliding window. A single decode step
/// needs none: its one query sees the whole cache, so the caller skips this.
public enum Mask {
    /// Row-major [queries][keys] values: 0 where the query may attend, -infinity elsewhere.
    ///
    /// `keys` may be smaller than `queries`: a sliding layer trims its cache to the window, so a
    /// prompt longer than `windowSize` has fewer cached keys than queries. A query whose own keys
    /// were trimmed away attends to the oldest key still held, which keeps the row finite — a row
    /// of pure -infinity would come out of the softmax as NaN and poison the rest of the pass.
    public static func causal(queries: Int, keys: Int, window: Int?) throws -> [Float] {
        guard queries > 0, keys > 0 else {
            throw PlamoError.invalidRequest(reason: "mask shape")
        }
        if let window, window <= 0 { throw PlamoError.invalidRequest(reason: "mask window") }
        var values = [Float](repeating: -Float.infinity, count: queries * keys)
        let offset = keys - queries
        for query in 0..<queries {
            let last = max(query + offset, 0)
            let first = window.map { max(0, last - $0 + 1) } ?? 0
            for key in first...last { values[query * keys + key] = 0 }
        }
        return values
    }
}
