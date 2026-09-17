import Foundation
import MCP
import ProAppsCore

extension ToolSpec {
  static let measurements: [ToolSpec] = [
    .init(
      name: "media_verify_video",
      description:
        "Decode every video frame in a local clip. Defaults to 30 seconds/1800 frames; longer verification requires explicit maximumDurationSeconds (up to 120) and, if needed, maximumFrames (up to 7200). End-of-stream required. Audio, visual effects and editor import are not verified. Deadline-limited offline child; no playback or source writes.",
      properties: [
        "path": string(), "maximumFrames": integer(1, MediaProbe.maximumVerificationFrames),
        "maximumDurationSeconds": .object([
          "type": .string("number"), "exclusiveMinimum": .double(0),
          "maximum": .double(MediaProbe.maximumVerificationSeconds),
        ]),
      ], required: ["path"], readOnly: true),
    .init(
      name: "audio_measure",
      description:
        "Decode a local audio-bearing clip to temporary mono PCM16/16000 Hz using macOS afconvert. Default duration budget is 30 seconds; explicit maximumDurationSeconds permits up to 120 seconds, keeping fixed byte/sample and subprocess limits. Measure whole-clip and requested-window RMS, peak and zero crossings. Mono conversion is not per-channel verification, LUFS or a robust pitch estimate. No playback; temporary PCM is removed.",
      properties: [
        "path": string(),
        "maximumDurationSeconds": .object([
          "type": .string("number"), "exclusiveMinimum": .double(0),
          "maximum": .double(PCMMeasurement.maximumDurationSeconds),
        ]),
        "windows": array(
          object(
            ["startSeconds": number, "durationSeconds": number],
            ["startSeconds", "durationSeconds"]), maximum: PCMMeasurement.maximumWindows, minimum: 0
        ),
      ], required: ["path", "windows"], readOnly: true),
    .init(
      name: "video_frame_measure",
      description:
        "Measure mean device-RGB values (0–1) in 1–8 selected decoded video frames/regions. Regions use top-left display-oriented pixels. Returns actual sample times and dimensions. Limited to 16 megapixels; no screenshots, image export or playback. Selected-region evidence only, not full-file verification.",
      properties: [
        "path": string(),
        "samples": array(
          object(
            [
              "timeSeconds": number,
              "region": object(
                ["x": number, "y": number, "width": number, "height": number],
                ["x", "y", "width", "height"]),
            ], ["timeSeconds"]), maximum: FrameProbe.maximumSamples),
      ], required: ["path", "samples"], readOnly: true),
  ]
}

extension NativeService {
  func measureLocalFile(name: String, path: String) async throws -> String {
    guard let spec = ToolSpec.measurements.first(where: { $0.name == name }) else {
      throw ProAppsError.invalid("Unknown measurement operation")
    }
    let data = try Files.read(Files.existing(path, extensions: ["json"]))
    let arguments = try JSONDecoder().decode(Value.self, from: data)
    try validate(arguments, schema: spec.tool.inputSchema)
    let measured: Value
    switch name {
    case "media_verify_video":
      struct Input: Decodable {
        let path: String
        let maximumFrames: Int?
        let maximumDurationSeconds: Double?
      }
      let input = try decode(Input.self, arguments)
      measured = try await Value(
        MediaProbe().verifyShortVideo(
          path: input.path,
          maximumFrames: input.maximumFrames ?? MediaProbe.defaultVerificationFrames,
          maximumDurationSeconds: input.maximumDurationSeconds
            ?? MediaProbe.defaultVerificationSeconds))
    case "audio_measure":
      struct Input: Decodable {
        let path: String
        let windows: [AudioWindow]
        let maximumDurationSeconds: Double?
      }
      let input = try decode(Input.self, arguments)
      measured = try await Value(
        AudioProbe().measure(
          path: input.path, windows: input.windows,
          maximumDurationSeconds: input.maximumDurationSeconds
            ?? PCMMeasurement.defaultDurationSeconds))
    case "video_frame_measure":
      struct Input: Decodable {
        let path: String
        let samples: [FrameSample]
      }
      let input = try decode(Input.self, arguments)
      measured = try await Value(FrameProbe().measure(path: input.path, samples: input.samples))
    default:
      throw ProAppsError.invalid("Unknown measurement operation")
    }
    return String(decoding: try JSONEncoder().encode(measured), as: UTF8.self)
  }

  func measurement(
    _ name: String, _ arguments: Value,
    execute: @Sendable (String, String) async throws -> Value
  ) async throws -> CallTool.Result {
    let request = try Files.reserveOutput(
      directory: FileManager.default.temporaryDirectory.path, name: "measurement.json", kind: .edit)
    defer {
      Cleanup.perform {
        try FileManager.default.removeItem(at: request.deletingLastPathComponent())
      }
    }
    _ = try Files.writeNew(JSONEncoder().encode(arguments), to: request.path, extensions: ["json"])
    return response([
      "measurement": try await execute(name, request.path), "sourceModified": .bool(false),
      "playbackPerformed": .bool(false),
    ])
  }
}
