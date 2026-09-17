import Foundation
import Testing

@testable import ProAppsCore

struct FrameTextTests {
  @Test func slightOCRBoxOverflowIsClippedAndInvalidBoxesAreRejected() throws {
    let clipped = try FrameProbe.textRectangle(
      CGRect(x: -0.25, y: 7, width: 20, height: 12), imageSize: CGSize(width: 100, height: 100))
    #expect(clipped == CGRect(x: 0, y: 7, width: 19.75, height: 12))
    #expect(throws: ProAppsError.self) {
      try FrameProbe.textRectangle(.infinite, imageSize: CGSize(width: 100, height: 100))
    }
    #expect(throws: ProAppsError.self) {
      try FrameProbe.textRectangle(
        CGRect(x: 120, y: 0, width: 10, height: 10), imageSize: CGSize(width: 100, height: 100))
    }
  }

  @Test func localOCRFindsVisibleTextAndMapsCroppedCoordinates() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ocr-test-\(UUID().uuidString)")
    try FileManager.default.createDirectory(
      at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    defer { do { try FileManager.default.removeItem(at: root) } catch { Issue.record(error) } }
    let source = try await ContinuousVideoFixture.make(in: root)
    let recipe = EditRecipe(
      clips: [
        .init(
          sourcePath: source.path, selection: .init(startSeconds: 0, durationSeconds: 1, rate: 1))
      ],
      video: .init(
        width: 640, height: 360, frameRate: 30, resizeMode: .fill,
        titles: [.init(text: "TEST", x: 40, y: 200, fontSize: 64)]))
    let rendered = try await NativeEditor().render(recipe, directory: root.path, name: "text.mp4")
    let before = try Data(contentsOf: URL(fileURLWithPath: rendered.outputPath))
    let samples: [FrameSample] = [
      .init(timeSeconds: 0.3),
      .init(timeSeconds: 0.3, region: .init(x: 20, y: 180, width: 300, height: 130)),
      .init(timeSeconds: 0.3, region: .init(x: 350, y: 20, width: 200, height: 100)),
    ]
    let results = try await FrameProbe().recognizeText(path: rendered.outputPath, samples: samples)
    try #require(results.count == 3)
    let full = try #require(results[0].lines.first { $0.text.contains("TEST") })
    let cropped = try #require(results[1].lines.first { $0.text.contains("TEST") })
    #expect(results[0].actualTimeSeconds == 0.3)
    #expect(full.confidence > 0 && full.confidence <= 1)
    #expect(full.region.x > 30 && full.region.x < 80)
    #expect(full.region.y > 190 && full.region.y < 260)
    #expect(abs(full.region.x - cropped.region.x) < 5)
    #expect(abs(full.region.y - cropped.region.y) < 5)
    #expect(results[2].lines.isEmpty)
    struct Input: Codable {
      let path: String
      let samples: [FrameSample]
    }
    let request = root.appendingPathComponent("request.json")
    try JSONEncoder().encode(Input(path: rendered.outputPath, samples: samples)).write(
      to: request, options: .withoutOverwriting)
    let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent().deletingLastPathComponent()
    var environment = ProcessInfo.processInfo.environment
    environment["LLVM_PROFILE_FILE"] =
      package.appendingPathComponent(".build/debug/codecov/ocr-%p-%m.profraw").path
    let process = try await Runner.run(
      package.appendingPathComponent(".build/debug/apple-pro-apps"),
      ["measure-media", "video_text_recognize", request.path], environment: environment)
    try #require(process.status == 0)
    let cli = try JSONDecoder().decode([FrameTextMeasurement].self, from: Data(process.stdout.utf8))
    #expect(cli.first?.lines.contains { $0.text.contains("TEST") } == true)
    #expect(try Data(contentsOf: URL(fileURLWithPath: rendered.outputPath)) == before)
    await #expect(throws: ProAppsError.self) {
      try await FrameProbe().recognizeText(path: rendered.outputPath, samples: [])
    }
    await #expect(throws: ProAppsError.self) {
      try await FrameProbe().recognizeText(
        path: rendered.outputPath, samples: [.init(timeSeconds: 2)])
    }
    await #expect(throws: ProAppsError.self) {
      try await FrameProbe().recognizeText(
        path: rendered.outputPath,
        samples: [.init(timeSeconds: 0, region: .init(x: -1, y: 0, width: 20, height: 20))])
    }
  }
}
