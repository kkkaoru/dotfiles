import AVFoundation
import Foundation
import Vision

/// Sampled display-oriented rectangles, not identity or exhaustive tracking.
public struct FrameHeadMeasurement: Codable, Sendable {
  public let requestedTimeSeconds: Double
  public let actualTimeSeconds: Double
  public let width: Int
  public let height: Int
  public let faces: [EditCrop]
  public let bodies: [EditCrop]
}

extension FrameProbe {
  /// Detect faces and whole people locally. Empty arrays retain missing detections.
  /// Uses the same bounded, exact-time decoder as OCR; never chooses a subject.
  public func detectHeads(path: String, samples: [FrameSample]) async throws
    -> [FrameHeadMeasurement]
  {
    guard samples.allSatisfy({ $0.region == nil }) else {
      throw ProAppsError.invalid("Head detection requires full-frame samples")
    }
    let generator = try await source(path: path, samples: samples)
    var result: [FrameHeadMeasurement] = []
    for sample in samples {
      try Task.checkCancellation()
      let frame = try await generator.image(
        at: CMTime(seconds: sample.timeSeconds, preferredTimescale: EditPlan.timeScale))
      let size = CGSize(width: frame.image.width, height: frame.image.height)
      let faces = try await DetectFaceRectanglesRequest().perform(on: frame.image)
      var request = DetectHumanRectanglesRequest()
      request.upperBodyOnly = false
      let bodies = try await request.perform(on: frame.image)
      try Task.checkCancellation()
      result.append(
        FrameHeadMeasurement(
          requestedTimeSeconds: sample.timeSeconds, actualTimeSeconds: frame.actualTime.seconds,
          width: frame.image.width, height: frame.image.height,
          faces: try Self.headRegions(faces, size: size),
          bodies: try Self.headRegions(bodies, size: size))
      )
    }
    return result
  }

  static func headRegions<Observation: BoundingBoxProviding>(
    _ observations: [Observation], size: CGSize
  ) throws -> [EditCrop] {
    guard observations.count <= 32 else { throw ProAppsError.outputLimit }
    return try observations.map { observation in
      let box = try textRectangle(
        observation.boundingBox.toImageCoordinates(size, origin: .upperLeft), imageSize: size)
      return EditCrop(x: box.minX, y: box.minY, width: box.width, height: box.height)
    }
  }
}
