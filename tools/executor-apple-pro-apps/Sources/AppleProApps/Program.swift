import Foundation
import ProAppsCore

struct ProgramRuntime: Sendable {
  let binary: URL
  let environment: [String: String]
  let serve: @Sendable () async throws -> Void
  let inventory: @Sendable () async throws -> String
  let ui: @Sendable (Configuration) throws -> Void
  let execute: @Sendable (URL, [String], [String: String]) async throws -> String
  var measureMedia: @Sendable (String, String) async throws -> String = { name, path in
    try await NativeService().measureLocalFile(name: name, path: path)
  }
  var editMedia: @Sendable (String) async throws -> String = { path in
    try await NativeService().renderLocalFile(path)
  }
  var inspectMedia: @Sendable (String) async throws -> String = { path in
    let summary = try await MediaProbe().inspect(path: path)
    return String(decoding: try JSONEncoder().encode(summary), as: UTF8.self)
  }
}

struct Program {
  let runtime: ProgramRuntime

  func run(_ command: Command) async throws -> String? {
    switch command {
    case .serve:
      try await runtime.serve()
      return nil
    case .capabilities:
      return try await runtime.inventory()
    case .inspectMedia(let path):
      return try await runtime.inspectMedia(path)
    case .measureMedia(let name, let path):
      return try await runtime.measureMedia(name, path)
    case .editMedia(let path):
      return try await runtime.editMedia(path)
    case .ui(let configuration):
      try configuration.validate(requireUI: true)
      try runtime.ui(configuration)
      return nil
    case .setup(let configuration):
      try configuration.validate(requireUI: configuration.withUI)
      let environment = configuration.environment(from: runtime.environment)
      let registration = Registration(approve: configuration.approve) { arguments in
        try await runtime.execute(configuration.executor, arguments, environment)
      }
      try await registration.register(binary: runtime.binary, repo: configuration.repo)
      if configuration.withUI {
        try await registration.register(
          binary: runtime.binary, repo: configuration.repo, ui: true,
          peekaboo: configuration.peekaboo)
      }
      return
        "Registered native Apple Pro Apps MCP\(configuration.withUI ? " and optional UI fallback" : ""). Verify tools and health through Executor; registration is not application/permission validation."
    }
  }
}
