import Foundation
import Testing
import Vision

@testable import ProAppsCore

struct FrameHeadTests {
  private struct Observation: BoundingBoxProviding {
    let boundingBox: NormalizedRect
  }

  @Test func mapsObservedRectanglesWithoutChoosingOrInventingASubject() throws {
    let regions = try FrameProbe.headRegions(
      [
        Observation(
          boundingBox: NormalizedRect(
            normalizedRect: CGRect(x: 0.125, y: 0.25, width: 0.25, height: 0.5))),
        Observation(
          boundingBox: NormalizedRect(
            normalizedRect: CGRect(x: 0.5, y: 0.125, width: 0.25, height: 0.125))),
      ],
      size: CGSize(width: 100, height: 100))
    try #require(regions.count == 2)
    #expect(regions[0].x == 12.5 && regions[0].y == 25)
    #expect(regions[0].width == 25 && regions[0].height == 50)
    #expect(regions[1].x == 50 && regions[1].y == 75)
    #expect(regions[1].width == 25 && regions[1].height == 12.5)
    #expect(
      try FrameProbe.headRegions([Observation](), size: CGSize(width: 100, height: 100)).isEmpty)
    #expect(throws: ProAppsError.self) {
      try FrameProbe.headRegions(
        Array(
          repeating: Observation(
            boundingBox: NormalizedRect(normalizedRect: CGRect(x: 0, y: 0, width: 0.5, height: 0.5))
          ), count: 33),
        size: CGSize(width: 100, height: 100))
    }
    #expect(throws: ProAppsError.self) {
      try FrameProbe.headRegions(
        [
          Observation(
            boundingBox: NormalizedRect(normalizedRect: CGRect(x: 2, y: 0, width: 0.5, height: 0.5))
          )
        ],
        size: CGSize(width: 100, height: 100))
    }
  }

  @Test func missingHeadsRetainRealFrameTimesAndRejectCropsAndInvalidSamples() async throws {
    let source = try #require(
      Bundle.module.url(forResource: "black", withExtension: "mp4", subdirectory: "Fixtures"))
    let before = try Data(contentsOf: source)
    let probe = FrameProbe()
    let results = try await probe.detectHeads(path: source.path, samples: [.init(timeSeconds: 0)])
    let first = try #require(results.first)
    #expect(first.requestedTimeSeconds == 0 && first.actualTimeSeconds == 0)
    #expect(first.width == 16 && first.height == 16)
    #expect(first.faces.isEmpty && first.bodies.isEmpty)
    #expect(try Data(contentsOf: source) == before)
    await #expect(throws: ProAppsError.self) {
      try await probe.detectHeads(path: source.path, samples: [])
    }
    await #expect(throws: ProAppsError.self) {
      try await probe.detectHeads(path: source.path, samples: [.init(timeSeconds: 100)])
    }
    await #expect(throws: ProAppsError.self) {
      try await probe.detectHeads(
        path: source.path,
        samples: [.init(timeSeconds: 0, region: .init(x: 0, y: 0, width: 1, height: 1))])
    }
    await #expect(throws: CancellationError.self) {
      try await withThrowingTaskGroup(of: Void.self) { group in
        group.cancelAll()
        group.addTask {
          _ = try await probe.detectHeads(path: source.path, samples: [.init(timeSeconds: 0)])
        }
        try await group.waitForAll()
      }
    }
  }
}
