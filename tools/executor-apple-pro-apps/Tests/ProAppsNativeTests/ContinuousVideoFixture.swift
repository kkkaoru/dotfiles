import AVFoundation
import CoreImage
import Foundation
import Testing

/// A time-varying opacity pass forces 30 render instants before color filtering.
/// Repeating slices of the same sample is coalesced by AVFoundation and is not a
/// continuous-frame fixture. Uses managed APIs, no pixel pointers.
enum ContinuousVideoFixture {
  static func make(in directory: URL) async throws -> URL {
    let seed = try #require(
      Bundle.module.url(forResource: "black", withExtension: "mp4", subdirectory: "Fixtures"))
    let asset = AVURLAsset(url: seed)
    let source = try #require(try await asset.loadTracks(withMediaType: .video).first)
    let range = CMTimeRange(start: .zero, duration: CMTime(value: 1, timescale: 1))
    let layer = AVMutableVideoCompositionLayerInstruction(assetTrack: source)
    layer.setOpacityRamp(fromStartOpacity: 0, toEndOpacity: 1, timeRange: range)
    let instruction = AVMutableVideoCompositionInstruction()
    instruction.timeRange = range
    instruction.layerInstructions = [layer]
    let cadence = AVMutableVideoComposition()
    cadence.frameDuration = CMTime(value: 1, timescale: 30)
    cadence.sourceTrackIDForFrameTiming = kCMPersistentTrackID_Invalid
    cadence.renderSize = CGSize(width: 16, height: 16)
    cadence.instructions = [instruction]
    let firstPass = try #require(
      AVAssetExportSession(asset: asset, presetName: AVAssetExportPresetHighestQuality))
    firstPass.videoComposition = cadence
    let continuous = directory.appendingPathComponent("continuous-black.mp4")
    try await firstPass.export(to: continuous, as: .mp4)
    let continuousAsset = AVURLAsset(url: continuous)
    let filters = try await AVVideoComposition.videoComposition(
      with: continuousAsset,
      applyingCIFiltersWithHandler: { request in
        let bounds = request.sourceImage.extent
        let halfWidth = bounds.width / 2
        let halfHeight = bounds.height / 2
        let red = CIImage(color: CIColor(red: 1, green: 0, blue: 0)).cropped(
          to: CGRect(x: 0, y: halfHeight, width: halfWidth, height: halfHeight))
        let green = CIImage(color: CIColor(red: 0, green: 1, blue: 0)).cropped(
          to: CGRect(x: halfWidth, y: halfHeight, width: halfWidth, height: halfHeight))
        let blue = CIImage(color: CIColor(red: 0, green: 0, blue: 1)).cropped(
          to: CGRect(x: 0, y: 0, width: halfWidth, height: halfHeight))
        let yellow = CIImage(color: CIColor(red: 1, green: 1, blue: 0)).cropped(
          to: CGRect(x: halfWidth, y: 0, width: halfWidth, height: halfHeight))
        request.finish(
          with: red.composited(over: green).composited(over: blue).composited(over: yellow),
          context: nil)
      })
    let session = try #require(
      AVAssetExportSession(asset: continuousAsset, presetName: AVAssetExportPresetHighestQuality))
    session.videoComposition = filters
    let destination = directory.appendingPathComponent("continuous-quadrants.mp4")
    try await session.export(to: destination, as: .mp4)
    return destination
  }
}
