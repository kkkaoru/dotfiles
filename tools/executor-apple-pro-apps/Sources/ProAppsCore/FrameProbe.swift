import AVFoundation
import CoreImage
import CoreImage.CIFilterBuiltins
import Dispatch
import Foundation

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

  public func measure(path: String, samples: [FrameSample]) async throws -> [FrameMeasurement] {
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
    let context = CIContext()
    var result: [FrameMeasurement] = []
    for sample in samples {
      try Task.checkCancellation()
      guard sample.timeSeconds.isFinite, sample.timeSeconds >= 0, sample.timeSeconds < duration,
        sample.timeSeconds <= 86400
      else { throw ProAppsError.invalid("Frame sample lies outside source duration") }
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
