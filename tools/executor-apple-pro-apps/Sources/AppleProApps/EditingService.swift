import Foundation
import MCP
import ProAppsCore

extension NativeService {
  /// CLI and MCP share schema enforcement. Reading stays on the service's
  /// dedicated executor, rather than blocking a cooperative task executor.
  func readEditRequest(_ path: String) throws -> EditRequest {
    let data = try Files.read(Files.existing(path, extensions: ["json"]))
    let value = try JSONDecoder().decode(Value.self, from: data)
    try validate(value, schema: ToolSpec.editRequest)
    let request = try decode(EditRequest.self, value)
    _ = try EditPlan.build(request.recipe)
    return request
  }

  func renderLocalFile(_ path: String) async throws -> String {
    try Task.checkCancellation()
    let request = try readEditRequest(path)
    let result = try await NativeEditor().render(
      request.recipe,
      directory: request.outputDirectory, name: request.outputName)
    return String(decoding: try JSONEncoder().encode(result), as: UTF8.self)
  }

  func editing(
    _ name: String, _ arguments: Value,
    render: @Sendable (String) async throws -> EditRenderResult
  ) async throws -> CallTool.Result {
    switch name {
    case "speech_cut_plan":
      let request = try decode(SpeechCutRequest.self, arguments)
      return response([
        "plan": try Value(SpeechCutPlan.make(request)), "sourceFilesValidated": .bool(false),
        "verificationScope": .string("provided-spans-and-timeline-only"),
      ])
    case "media_edit_plan":
      struct Input: Decodable { let recipe: EditRecipe }
      let input = try decode(Input.self, arguments)
      return response([
        "plan": try Value(EditPlan.build(input.recipe)),
        "sourceFilesValidated": .bool(false),
        "verificationScope": .string("recipe-shape-and-timeline-only"),
      ])
    case "media_project_read":
      struct Input: Decodable { let path: String }
      let request = try readEditRequest(decode(Input.self, arguments).path)
      return response([
        "request": try Value(request), "plan": try Value(EditPlan.build(request.recipe)),
        "sourceFilesValidated": .bool(false),
      ])
    case "media_edit":
      let request = try decode(EditRequest.self, arguments)
      _ = try EditPlan.build(request.recipe)
      let config = try Files.reserveOutput(
        directory: FileManager.default.temporaryDirectory.path,
        name: "request.json", kind: .edit)
      defer {
        Cleanup.perform {
          try FileManager.default.removeItem(at: config.deletingLastPathComponent())
        }
      }
      _ = try Files.writeNew(JSONEncoder().encode(request), to: config.path, extensions: ["json"])
      let result = try await render(config.path)
      return response([
        "render": try Value(result), "sourceWritePerformed": .bool(false),
        "fullMediaVerified": .bool(false),
        "verificationScope": .string("export-completion-and-track-metadata"),
        "retrySafe": .bool(false),
      ])
    default:
      throw ProAppsError.invalid("Unknown editing operation")
    }
  }
}
