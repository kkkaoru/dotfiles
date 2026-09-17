import AVFoundation
import CoreImage
import CoreImage.CIFilterBuiltins
import Dispatch
import Foundation
import Vision

public struct FrameSample: Codable, Sendable {
  public let timeSeconds: Double
  public let region: EditCrop?
  public init(timeSeconds: Double, region: EditCrop? = nil) {
    self.timeSeconds = timeSeconds
    self.region = region
  }
}

public struct FrameMeasurement: Codable, Sendable {
  public let requestedTimeSeconds: Double
  public let actualTimeSeconds: Double
  public let width: Int
  public let height: Int
  public let meanRed: Double
  public let meanGreen: Double
  public let meanBlue: Double
}

public struct FrameTextLine: Codable, Sendable {
  public let text: String
  public let confidence: Float
  public let boundsClipped: Bool
  public let region: EditCrop
}

public struct FrameTextMeasurement: Codable, Sendable {
  public let requestedTimeSeconds: Double
  public let actualTimeSeconds: Double
  public let width: Int
  public let height: Int
  public let lines: [FrameTextLine]
}

/// Managed CoreImage/CoreGraphics sampling, no screenshots or exported images.
/// Region coordinates are top-left pixels in the display-oriented decoded frame.
public actor FrameProbe {
  nonisolated private let executor = DispatchSerialQueue(label: "apple-pro-apps.frame-measure")
  nonisolated public var unownedExecutor: UnownedSerialExecutor {
    executor.asUnownedSerialExecutor()
  }
  public static let maximumSamples = 8
  private static let maximumPixels = 16_777_216.0

  public init() {}

  private func source(path: String, samples: [FrameSample]) async throws
    -> sending AVAssetImageGenerator
  {
    try Task.checkCancellation()
    guard (1...Self.maximumSamples).contains(samples.count) else {
      throw ProAppsError.invalid("Supply 1–8 frame samples")
    }
    let asset = AVURLAsset(url: try Files.existing(path))
    let duration = try await asset.load(.duration).seconds
    let tracks = try await asset.loadTracks(withMediaType: .video)
    guard let track = tracks.first, duration.isFinite, duration > 0 else {
      throw ProAppsError.invalid("Expected finite-duration video")
    }
    let size = try await track.load(.naturalSize)
    let preferred = try await track.load(.preferredTransform)
    try Self.validateGeometry(size: size, transform: preferred)
    let generator = AVAssetImageGenerator(asset: asset)
    generator.appliesPreferredTrackTransform = true
    generator.requestedTimeToleranceBefore = .zero
    generator.requestedTimeToleranceAfter = .zero
    for sample in samples {
      guard sample.timeSeconds.isFinite, sample.timeSeconds >= 0, sample.timeSeconds < duration,
        sample.timeSeconds <= 86400
      else { throw ProAppsError.invalid("Frame sample lies outside source duration") }
    }
    return generator
  }

  public func measure(path: String, samples: [FrameSample]) async throws -> [FrameMeasurement] {
    let generator = try await source(path: path, samples: samples)
    let context = CIContext()
    var result: [FrameMeasurement] = []
    for sample in samples {
      try Task.checkCancellation()
      let frame = try await generator.image(
        at: CMTime(seconds: sample.timeSeconds, preferredTimescale: EditPlan.timeScale))
      let image = CIImage(cgImage: frame.image)
      let region = try Self.region(
        sample.region, width: frame.image.width, height: frame.image.height)
      let color = try Self.average(image, region: region, context: context)
      result.append(
        FrameMeasurement(
          requestedTimeSeconds: sample.timeSeconds, actualTimeSeconds: frame.actualTime.seconds,
          width: frame.image.width, height: frame.image.height, meanRed: color.red,
          meanGreen: color.green, meanBlue: color.blue))
    }
    return result
  }

  /// Local Japanese/English OCR; confidence is an OCR score, not speech accuracy.
  /// Returned rectangles use full-frame top-left pixels even for cropped input.
  public func recognizeText(path: String, samples: [FrameSample]) async throws
    -> [FrameTextMeasurement]
  {
    let generator = try await source(path: path, samples: samples)
    let context = CIContext()
    let maximumLinesPerFrame = 64
    let maximumTextBytes = 32768
    var bytes = 0
    var result: [FrameTextMeasurement] = []
    for sample in samples {
      try Task.checkCancellation()
      let frame = try await generator.image(
        at: CMTime(seconds: sample.timeSeconds, preferredTimescale: EditPlan.timeScale))
      let selected = try Self.region(
        sample.region, width: frame.image.width, height: frame.image.height
      ).integral
      guard let bitmap = context.createCGImage(CIImage(cgImage: frame.image), from: selected) else {
        throw ProAppsError.unavailable("Cannot prepare the selected OCR frame")
      }
      var request = RecognizeTextRequest()
      request.recognitionLevel = .accurate
      request.recognitionLanguages = [
        Locale.Language(identifier: "ja-JP"), Locale.Language(identifier: "en-US"),
      ]
      request.usesLanguageCorrection = false
      let observations = try await request.perform(on: bitmap)
      guard observations.count <= maximumLinesPerFrame else { throw ProAppsError.outputLimit }
      var lines: [FrameTextLine] = []
      for observation in observations {
        try Task.checkCancellation()
        guard let candidate = observation.topCandidates(1).first else {
          throw ProAppsError.unavailable("OCR observation has no text candidate")
        }
        bytes += candidate.string.utf8.count
        guard bytes <= maximumTextBytes else { throw ProAppsError.outputLimit }
        let rawRectangle = observation.boundingBox.toImageCoordinates(
          CGSize(width: bitmap.width, height: bitmap.height), origin: .upperLeft)
        let rectangle = try Self.textRectangle(
          rawRectangle, imageSize: CGSize(width: bitmap.width, height: bitmap.height))
        let top = Double(frame.image.height) - selected.maxY
        lines.append(
          FrameTextLine(
            text: candidate.string, confidence: candidate.confidence,
            boundsClipped: rectangle != rawRectangle,
            region: .init(
              x: selected.minX + rectangle.minX, y: top + rectangle.minY,
              width: rectangle.width, height: rectangle.height)))
      }
      result.append(
        FrameTextMeasurement(
          requestedTimeSeconds: sample.timeSeconds, actualTimeSeconds: frame.actualTime.seconds,
          width: frame.image.width, height: frame.image.height, lines: lines))
    }
    return result
  }

  static func textRectangle(_ rectangle: CGRect, imageSize: CGSize) throws -> CGRect {
    guard !rectangle.isInfinite, !rectangle.isNull,
      rectangle.origin.x.isFinite, rectangle.origin.y.isFinite,
      rectangle.width.isFinite, rectangle.height.isFinite, rectangle.width > 0, rectangle.height > 0
    else { throw ProAppsError.invalid("Invalid OCR rectangle") }
    let clipped = rectangle.intersection(CGRect(origin: .zero, size: imageSize))
    guard !clipped.isNull, !clipped.isEmpty else {
      throw ProAppsError.invalid("OCR rectangle lies outside the selected image")
    }
    return clipped
  }

  static func validateGeometry(size: CGSize, transform: CGAffineTransform) throws {
    let maximumDimension = 16384.0
    guard size.width.isFinite, size.height.isFinite, size.width > 0, size.height > 0,
      size.width <= maximumDimension, size.height <= maximumDimension,
      size.width * size.height <= maximumPixels,
      [transform.a, transform.b, transform.c, transform.d, transform.tx, transform.ty].allSatisfy(
        \.isFinite),
      abs(transform.tx) <= maximumDimension, abs(transform.ty) <= maximumDimension
    else { throw ProAppsError.invalid("Frame measurement geometry budget exceeded") }
    let display = CGRect(origin: .zero, size: size).applying(transform)
    guard display.width.isFinite, display.height.isFinite, display.width > 0, display.height > 0,
      display.width <= maximumDimension, display.height <= maximumDimension,
      display.width * display.height <= maximumPixels
    else { throw ProAppsError.invalid("Display transform exceeds the frame measurement budget") }
  }

  static func region(_ requested: EditCrop?, width: Int, height: Int) throws -> CGRect {
    guard let requested else { return CGRect(x: 0, y: 0, width: width, height: height) }
    guard requested.x.isFinite, requested.y.isFinite, requested.width.isFinite,
      requested.height.isFinite,
      requested.x >= 0, requested.y >= 0, requested.width > 0, requested.height > 0,
      requested.x + requested.width <= Double(width),
      requested.y + requested.height <= Double(height)
    else { throw ProAppsError.invalid("Frame measurement region is out of bounds") }
    return CGRect(
      x: requested.x, y: Double(height) - requested.y - requested.height, width: requested.width,
      height: requested.height)
  }

  static func average(_ image: CIImage, region: CGRect, context: CIContext) throws -> (
    red: Double, green: Double, blue: Double
  ) {
    let filter = CIFilter.areaAverage()
    filter.inputImage = image
    filter.extent = region
    guard let mean = filter.outputImage,
      let bitmap = context.createCGImage(
        mean, from: CGRect(x: 0, y: 0, width: 1, height: 1), format: .RGBA8,
        colorSpace: CGColorSpaceCreateDeviceRGB()),
      let storage = bitmap.dataProvider?.data
    else { throw ProAppsError.unavailable("Cannot measure decoded frame pixels") }
    let bytes = [UInt8](storage as Data)
    guard bytes.count >= 4 else { throw ProAppsError.unavailable("Incomplete RGBA measurement") }
    let channelMaximum = 255.0
    return (
      Double(bytes[0]) / channelMaximum, Double(bytes[1]) / channelMaximum,
      Double(bytes[2]) / channelMaximum
    )
  }
}
