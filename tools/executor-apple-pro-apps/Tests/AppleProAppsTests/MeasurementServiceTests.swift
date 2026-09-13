import Foundation
import MCP
import ProAppsCore
import Testing

@testable import AppleProApps

struct MeasurementServiceTests {
  private actor Capture {
    var path: String?
    func record(_ path: String) { self.path = path }
  }

  @Test(arguments: ["success", "failure", "cancelled"])
  func requestFilesArePrivateAndRemovedOnAllOutcomes(_ mode: String) async throws {
    let capture = Capture()
    var interfaces = NativeInterfaces()
    interfaces.measureMedia = { name, path in
      await capture.record(path)
      #expect(name == "media_verify_video")
      let value = try JSONDecoder().decode(Value.self, from: Files.read(URL(fileURLWithPath: path)))
      #expect(value.objectValue?["path"] == .string("/tmp/synthetic.mp4"))
      #expect(value.objectValue?["maximumDurationSeconds"] == .int(60))
      #expect(value.objectValue?["maximumFrames"] == .int(7200))
      if mode == "failure" { throw ProAppsError.unavailable("Synthetic failure") }
      if mode == "cancelled" { throw CancellationError() }
      return .object(["fullVideoDecoded": .bool(true)])
    }
    let result = await NativeService(interfaces: interfaces).call(
      .init(
        name: "media_verify_video",
        arguments: [
          "path": .string("/tmp/synthetic.mp4"), "maximumDurationSeconds": .int(60),
          "maximumFrames": .int(7200),
        ]))
    #expect(result.isError == (mode != "success"))
    let path = try #require(await capture.path)
    #expect(
      !FileManager.default.fileExists(
        atPath: URL(fileURLWithPath: path).deletingLastPathComponent().path))
    if mode == "success" {
      #expect(result.structuredContent?.objectValue?["sourceModified"] == .bool(false))
      #expect(
        result.structuredContent?.objectValue?["measurement"]?.objectValue?["fullVideoDecoded"]
          == .bool(true))
    }
  }

  @Test(arguments: ["media_verify_video", "audio_measure", "video_frame_measure"])
  func nativeMeasurementCLIUsesTheSameStrictSchema(_ name: String) async throws {
    let request = try Files.reserveOutput(
      directory: FileManager.default.temporaryDirectory.path, name: "request.json", kind: .edit)
    let root = request.deletingLastPathComponent()
    defer { do { try FileManager.default.removeItem(at: root) } catch { Issue.record(error) } }
    let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent().deletingLastPathComponent()
    let video = package.appendingPathComponent("Tests/ProAppsNativeTests/Fixtures/black.mp4")
    var arguments: [String: Value] = ["path": .string(video.path)]
    if name == "audio_measure" {
      // One second of silent mono PCM16 at 16000 Hz; literal RIFF header.
      let header: [UInt8] = [
        82, 73, 70, 70, 36, 125, 0, 0, 87, 65, 86, 69, 102, 109, 116, 32, 16, 0, 0, 0, 1, 0, 1, 0,
        128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0, 100, 97, 116, 97, 0, 125, 0, 0,
      ]
      let audio = root.appendingPathComponent("silence.wav")
      try (Data(header) + Data(repeating: 0, count: 32000)).write(
        to: audio, options: .withoutOverwriting)
      arguments = ["path": .string(audio.path), "windows": .array([])]
    } else if name == "video_frame_measure" {
      arguments["samples"] = .array([.object(["timeSeconds": .int(0)])])
    }
    _ = try Files.writeNew(
      JSONEncoder().encode(Value.object(arguments)), to: request.path, extensions: ["json"])
    let service = NativeService()
    let text = try await service.measureLocalFile(name: name, path: request.path)
    let result = try JSONDecoder().decode(Value.self, from: Data(text.utf8))
    if name == "media_verify_video" {
      #expect(result.objectValue?["decodedFrames"] == .int(1))
      #expect(result.objectValue?["fullVideoDecoded"] == .bool(true))
      let extended = try Files.writeNew(
        JSONEncoder().encode(
          Value.object([
            "path": .string(video.path), "maximumDurationSeconds": .int(60),
            "maximumFrames": .int(7200),
          ])), to: root.appendingPathComponent("extended.json").path, extensions: ["json"])
      let extendedText = try await service.measureLocalFile(name: name, path: extended.path)
      #expect(try JSONDecoder().decode(Value.self, from: Data(extendedText.utf8)) == result)
    } else if name == "audio_measure" {
      #expect(result.objectValue?["whole"]?.objectValue?["frames"] == .int(16000))
    } else {
      #expect(result.arrayValue?.count == 1)
    }
    let binary = package.appendingPathComponent(".build/debug/apple-pro-apps")
    var environment = ProcessInfo.processInfo.environment
    environment["LLVM_PROFILE_FILE"] =
      package.appendingPathComponent(".build/debug/codecov/measurement-%p-%m.profraw").path
    let process = try await Runner.run(
      binary, ["measure-media", name, request.path], environment: environment, timeout: .seconds(15)
    )
    #expect(process.status == 0)
    #expect(try JSONDecoder().decode(Value.self, from: Data(process.stdout.utf8)) == result)
    arguments["shell"] = .string("refused")
    let bad = try Files.writeNew(
      JSONEncoder().encode(Value.object(arguments)),
      to: root.appendingPathComponent("bad.json").path, extensions: ["json"])
    await #expect(throws: (any Error).self) {
      try await service.measureLocalFile(name: name, path: bad.path)
    }
  }

  @Test func malformedCLIAndUnknownOperationFailClosed() async {
    #expect(throws: (any Error).self) { try Command.parse(["measure-media"]) }
    await #expect(throws: (any Error).self) {
      try await NativeService().measureLocalFile(name: "unknown", path: "/tmp/none.json")
    }
  }
}
