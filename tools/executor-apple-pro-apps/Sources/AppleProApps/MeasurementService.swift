import Foundation
import MCP
import ProAppsCore

extension ToolSpec {
  static let measurements: [ToolSpec] = [
    .init(
      name: "audio_cue_track",
      description:
        "Generate one local mono PCM16/16kHz WAV track of up to 120 seconds with up to 120 ordered, nonoverlapping caption-onset sounds. Each sound is an 80ms 880Hz sine with a smooth envelope, gain 0–0.25. Output seconds round to PCM samples. No playback, recording or downloaded assets. Creates a private edit directory and cue-request.json without replacing files. Use one additionalAudio layer to mix it with source voice; allow headroom. This is sound generation, not audio measurement.",
      properties: [
        "outputDirectory": string(), "outputName": string(maximum: 180),
        "track": object(
          [
            "durationSeconds": number, "gain": number,
            "onsetSeconds": array(number, maximum: CueSound.maximumCues, minimum: 0),
          ], ["durationSeconds", "gain", "onsetSeconds"]),
      ],
      required: ["outputDirectory", "outputName", "track"], readOnly: false),
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
    Self(
      name: "speech_locale_reserve",
      description:
        "Explicitly reserve a supported SpeechTranscriber locale for this application on macOS 26+. Persistent app-scoped resource change: requires user approval. Does not download, install or release models/locales. Existing reservations are retained; full quota fails without eviction. Returns whether already-present assets are ready for this app. A locale listing alone does not prove readiness. Call before audio_transcribe when preparing an approved locale.",
      properties: ["locale": string(maximum: 64)], required: ["locale"], readOnly: false),
    .init(
      name: "audio_transcribe",
      description:
        "Transcribe an existing local M4A/WAV/AIFF/CAF audio file of at most 60 seconds using macOS 26 SpeechAnalyzer and an already-installed locale model. No download, microphone capture or cloud fallback. Returns final text with approximate phrase-level timings by default. Opt-in wordTiming returns native audio-time-indexed text runs instead of character-proportional timing; missing lexical timing is rejected. Optional contextualStrings supplies up to 128 explicit local vocabulary hints (8192 UTF-8 bytes total), not automatic text correction. No volatile/fast results, speaker identification, guaranteed word alignment or human-reviewed accuracy. Empty segments mean no recognized speech. 45-second analysis watchdog within a 60-second disposable child deadline.",
      properties: [
        "path": string(), "locale": string(maximum: 64), "wordTiming": boolean,
        "contextualStrings": array(string(maximum: 128), maximum: 128, minimum: 0),
      ], required: ["path", "locale"],
      readOnly: true),
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
    case "audio_cue_track":
      measured = try createCueTrack(arguments)
    case "speech_locale_reserve":
      struct Input: Decodable { let locale: String }
      let input = try decode(Input.self, arguments)
      measured = try await Value(SpeechProbe().reserve(locale: input.locale))
    case "audio_transcribe":
      struct Input: Decodable {
        let path: String
        let locale: String
        let wordTiming: Bool?
        let contextualStrings: [String]?
      }
      let input = try decode(Input.self, arguments)
      let options = try SpeechRecognitionOptions(
        wordTiming: input.wordTiming ?? false, contextualStrings: input.contextualStrings ?? [])
      measured = try await Value(
        SpeechProbe().transcribe(path: input.path, locale: input.locale, options: options))
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

  private func createCueTrack(_ arguments: Value) throws -> Value {
    struct Input: Decodable {
      let outputDirectory: String
      let outputName: String
      let track: CueSound
    }
    let input = try decode(Input.self, arguments)
    guard URL(fileURLWithPath: input.outputName).pathExtension.lowercased() == "wav" else {
      throw ProAppsError.invalid("Cue track output requires a WAV extension")
    }
    let wave = try input.track.wave()
    let output = try Files.reserveOutput(
      directory: input.outputDirectory, name: input.outputName, kind: .edit)
    do {
      let project = output.deletingLastPathComponent().appendingPathComponent("cue-request.json")
      _ = try writeCueArtifact(JSONEncoder().encode(input.track), to: project.path)
      try Task.checkCancellation()
      _ = try writeCueArtifact(wave, to: output.path)
      return .object([
        "outputPath": .string(output.path), "projectPath": .string(project.path),
        "cueCount": .int(input.track.onsetSeconds.count), "generatedTone": .bool(true),
      ])
    } catch {
      Cleanup.perform { try FileManager.default.removeItem(at: output.deletingLastPathComponent()) }
      throw error
    }
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
