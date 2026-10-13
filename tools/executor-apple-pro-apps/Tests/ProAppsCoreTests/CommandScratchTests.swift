import Foundation
import Testing

@testable import ProAppsCore

struct CommandScratchTests {
  @Test func explicitScratchIsCanonicalAndNeverSilentlyFallsBack() throws {
    let path = try #require(ProcessInfo.processInfo.environment["TMPDIR"])
    let directory = try CommandScratch.directory(environment: ["TMPDIR": path])
    #expect(directory.path == URL(fileURLWithPath: path).path)
    #expect(directory.resolvingSymlinksInPath().path == directory.path)
    #expect(
      try CommandScratch.directory(environment: [:]) == FileManager.default.temporaryDirectory)
    #expect(throws: ProAppsError.self) {
      try CommandScratch.directory(environment: ["TMPDIR": "relative"])
    }
    #expect(throws: ProAppsError.self) {
      try CommandScratch.directory(environment: ["TMPDIR": path + "/../tmp"])
    }
    let link = URL(fileURLWithPath: path).appendingPathComponent(
      "scratch-link-\(UUID().uuidString)")
    try FileManager.default.createSymbolicLink(atPath: link.path, withDestinationPath: path)
    defer { do { try FileManager.default.removeItem(at: link) } catch { Issue.record(error) } }
    #expect(throws: ProAppsError.self) {
      try CommandScratch.directory(environment: ["TMPDIR": link.path])
    }
    #expect(throws: (any Error).self) {
      try CommandScratch.directory(environment: ["TMPDIR": link.path + "/missing"])
    }
  }

  @Test func runnerChecksOutputAndTerminatesAnUncooperativeChild() async throws {
    #expect(try await Runner.checked(URL(fileURLWithPath: "/usr/bin/printf"), ["ok"]) == "ok")
    await #expect(throws: ProAppsError.unavailable("Native output was not UTF-8")) {
      try await Runner.run(URL(fileURLWithPath: "/usr/bin/printf"), ["\\377"])
    }
    await #expect(throws: ProAppsError.timedOut) {
      try await Runner.run(
        URL(fileURLWithPath: "/bin/sh"), ["-c", "trap '' TERM; exec sleep 5"],
        timeout: .milliseconds(100))
    }
    let signal = ExitSignal()
    signal.finish()
    signal.finish()
    await signal.wait()
  }
}
