import Foundation
import MLX

/// Rotary embedding for one attention flavour: mlx_lm's plamo3 uses plain RoPE, with theta 1e6
/// on the full-attention layers and 1e4 on the sliding ones. It ignores `rope_parameters`' yarn
/// scaling, so matching its output means ignoring it here too.
public final class Rotary {
    /// `base ** (-2i / headDim)` for every rotary pair.
    public let invFreq: MLXArray
    // The table covers every position the cache can reach, so a decode step slices it instead of
    // recomputing sin/cos over the whole window for all 16 layers on every token.
    private var cosTable: MLXArray
    private var sinTable: MLXArray

    /// Builds the table for one flavour; `headDim` is the model's, not the cache's.
    public init(_ parameters: PlamoRopeParameters, headDim: Int) {
        let half = max(headDim / 2, 1)
        // MLX's RoPE uses base ** (arange(0, dim, 2) / dim), i.e. 2 * i / headDim.
        let exponent = MLX.arange(0, half, dtype: .float32) * 2 / Float(max(headDim, 2))
        invFreq = 1 / pow(MLXArray(parameters.ropeTheta), exponent)
        (cosTable, sinTable) = Rotary.table(invFreq: invFreq, count: 1)
    }

    private static func table(invFreq: MLXArray, count: Int) -> (MLXArray, MLXArray) {
        let positions = MLXArray((0..<count).map { Float($0) })
        let freqs = positions.expandedDimensions(axis: -1) * invFreq.expandedDimensions(axis: 0)
        let embedded = concatenated([freqs, freqs], axis: -1)
        return (cos(embedded), sin(embedded))
    }

    /// How many positions the table holds; the growth keeps a long context off the hot path.
    public var capacity: Int { cosTable.dim(0) }

    /// cos and sin for `positions`, shaped [1, 1, positions.count, headDim] to broadcast over
    /// heads.
    /// An empty range is a programming error in the caller, not a value to paper over.
    public func cosSin(positions: [Int]) throws -> (MLXArray, MLXArray) {
        guard let first = positions.first, let last = positions.last, first >= 0, last >= first
        else {
            throw PlamoError.invalidRequest(reason: "rope positions")
        }
        if last >= cosTable.dim(0) {
            (cosTable, sinTable) = Rotary.table(
                invFreq: invFreq, count: max(last + 1, cosTable.dim(0) * 2))
        }
        let shape = [1, 1, positions.count, invFreq.dim(0) * 2]
        // Every caller passes a contiguous range: the new tokens, or the cache's tail.
        return (
            cosTable[first..<(last + 1)].reshaped(shape),
            sinTable[first..<(last + 1)].reshaped(shape)
        )
    }
}
