import AVFoundation
import CoreImage
import Foundation
import Testing

@testable import ProAppsCore

struct VideoAlphaLayerTests {
  @Test func proResAlphaRetainsBackgroundAndBlendsTranslucentPixels() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "video-alpha-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
    defer { do { try FileManager.default.removeItem(at: root) } catch { Issue.record(error) } }
    let source = try await ContinuousVideoFixture.make(in: root)
    let before = try Data(contentsOf: source)
    let asset = AVURLAsset(url: source)
    let filters = try await AVVideoComposition.videoComposition(
      with: asset,
      applyingCIFiltersWithHandler: { request in
        let bounds = request.sourceImage.extent
        let clear = CIImage(color: CIColor(red: 0, green: 0, blue: 0, alpha: 0))
          .cropped(to: bounds)
        let patch = CIImage(color: CIColor(red: 0, green: 0, blue: 1, alpha: 0.5))
          .cropped(
            to: CGRect(
              x: bounds.midX, y: bounds.midY, width: bounds.width / 2, height: bounds.height / 2))
        request.finish(with: patch.composited(over: clear), context: nil)
      })
    let export = try #require(
      AVAssetExportSession(asset: asset, presetName: AVAssetExportPresetAppleProRes4444LPCM))
    export.videoComposition = filters
    let alpha = root.appendingPathComponent("synthetic-alpha.mov")
    try await export.export(to: alpha, as: .mov)
    let alphaBefore = try Data(contentsOf: alpha)
    let recipe = EditRecipe(
      clips: [
        .init(
          sourcePath: source.path,
          selection: .init(startSeconds: 0, durationSeconds: 1, rate: 1))
      ],
      video: .init(width: 64, height: 64, frameRate: 30, resizeMode: .fit),
      additionalVideo: [
        .init(
          sourcePath: alpha.path,
          selection: .init(startSeconds: 0, durationSeconds: 1, rate: 1), offsetSeconds: 0)
      ])
    let rendered = try await NativeEditor().render(recipe, directory: root.path, name: "alpha.mp4")
    let verified = try await MediaProbe().verifyShortVideo(path: rendered.outputPath)
    #expect(verified.decodedFrames == 30)
    #expect(verified.fullVideoDecoded)
    #expect(rendered.audioTrackCount == 0)
    let pixels = try await FrameProbe().measure(
      path: rendered.outputPath,
      samples: [
        .init(timeSeconds: 0.5, region: .init(x: 8, y: 8, width: 8, height: 8)),
        .init(timeSeconds: 0.5, region: .init(x: 48, y: 8, width: 8, height: 8)),
        .init(timeSeconds: 0.5, region: .init(x: 8, y: 48, width: 8, height: 8)),
      ])
    try #require(pixels.count == 3)
    #expect(pixels[0].meanRed > 0.8 && pixels[0].meanBlue < 0.1)
    #expect(pixels[1].meanGreen > 0.2 && pixels[1].meanBlue > 0.2 && pixels[1].meanRed < 0.1)
    #expect(pixels[2].meanBlue > 0.8 && pixels[2].meanRed < 0.1)
    #expect(try Data(contentsOf: source) == before)
    #expect(try Data(contentsOf: alpha) == alphaBefore)
  }
}
