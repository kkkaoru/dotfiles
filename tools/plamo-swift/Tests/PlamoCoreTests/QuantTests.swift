import MLX
import Testing

@testable import PlamoCore

@Test func aDenseMatrixMultiplies() {
    let weight = MLXArray([Float](repeating: 0.5, count: 4 * 3)).reshaped([4, 3])
    let quant = Quant(dense: weight)
    #expect(quant.activationDType == .float32)
    let x = MLXArray([Float](repeating: 2, count: 3)).reshaped([1, 3])
    let out = quant.matmul(x)
    MLX.eval(out)
    #expect(out.shape == [1, 4])
    #expect(out[0, 0].item(Float.self) == 3)
}

@Test func aQuantizedMatrixMultipliesAndEmbeds() throws {
    let weight = MLXArray((0..<(64 * 64)).map { Float($0 % 17) / 17 }).reshaped([64, 64])
    let (quantized, scales, biases) = MLX.quantized(
        weight, groupSize: Quant.groupSize, bits: Quant.bits)
    let quant = try Quant(
        arrays: [
            "w.weight": quantized, "w.scales": scales, "w.biases": try #require(biases),
        ], prefix: "w")
    #expect(quant.activationDType == scales.dtype)
    let x = MLXArray([Float](repeating: 1, count: 64)).reshaped([1, 64])
    let out = quant.matmul(x)
    MLX.eval(out)
    #expect(out.shape == [1, 64])
    let rows = quant.embed(MLXArray([Int32(0), 3]))
    MLX.eval(rows)
    #expect(rows.shape == [2, 64])
}

@Test func reportsAMissingWeightByName() {
    #expect(throws: PlamoError.self) { try Quant(arrays: [:], prefix: "missing") }
}
