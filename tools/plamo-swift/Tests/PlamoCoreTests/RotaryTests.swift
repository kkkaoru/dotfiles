import Foundation
import MLX
import Testing

@testable import PlamoCore

private func parameters(theta: Float) throws -> PlamoRopeParameters {
    let json = "{\"rope_type\":\"default\",\"rope_theta\":\(theta)}"
    return try JSONDecoder().decode(PlamoRopeParameters.self, from: Data(json.utf8))
}

@Test func buildsAPositionTableForTheFirstToken() throws {
    let rotary = Rotary(try parameters(theta: 10_000), headDim: 4)
    #expect(rotary.capacity == 1)
    let (cosValues, sinValues) = try rotary.cosSin(positions: [0])
    MLX.eval(cosValues, sinValues)
    #expect(cosValues.shape == [1, 1, 1, 4])
    #expect(sinValues.shape == [1, 1, 1, 4])
    #expect(cosValues[0, 0, 0, 0].item(Float.self) == 1)
}

@Test func growsTheTablePastItsCapacity() throws {
    let rotary = Rotary(try parameters(theta: 10_000), headDim: 4)
    let (cosValues, _) = try rotary.cosSin(positions: [0, 1, 2, 3])
    MLX.eval(cosValues)
    #expect(cosValues.shape == [1, 1, 4, 4])
    #expect(rotary.capacity >= 4)
    // The grown table still answers the same positions.
    let (again, _) = try rotary.cosSin(positions: [3])
    MLX.eval(again)
    #expect(again.shape == [1, 1, 1, 4])
}

@Test func rejectsAnEmptyOrNegativeRange() throws {
    let rotary = Rotary(try parameters(theta: 10_000), headDim: 4)
    #expect(throws: PlamoError.self) { try rotary.cosSin(positions: []) }
    #expect(throws: PlamoError.self) { try rotary.cosSin(positions: [-1]) }
    #expect(throws: PlamoError.self) { try rotary.cosSin(positions: [2, 1]) }
}

@Test func aLargerThetaChangesTheFrequencies() throws {
    let slow = Rotary(try parameters(theta: 1_000_000), headDim: 8)
    let fast = Rotary(try parameters(theta: 10_000), headDim: 8)
    MLX.eval(slow.invFreq, fast.invFreq)
    #expect(slow.invFreq[1].item(Float.self) != fast.invFreq[1].item(Float.self))
}
