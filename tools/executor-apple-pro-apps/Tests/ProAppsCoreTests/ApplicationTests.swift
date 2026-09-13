import Foundation
import Testing

@testable import ProAppsCore

@MainActor
struct ApplicationTests {
  @Test(arguments: [
    (ProApp.motion, "motn"), (.compressor, "compressor"), (.finalCutPro, "fcpxml"),
    (.logicPro, "mid"), (.mainStage, "concert"),
  ])
  func supportedDocumentTypes(_ app: ProApp, _ expected: String) {
    #expect(app.documentExtensions.contains(expected))
  }

  @Test(arguments: [true, false])
  func resolvesOnlyValidatedEditionsAndNativeRecipients(_ includeVersion: Bool) async throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
      "pro-apps-bundle-\(UUID().uuidString)")
    let app = directory.appendingPathComponent("Compressor.app")
    let contents = app.appendingPathComponent("Contents")
    try FileManager.default.createDirectory(at: contents, withIntermediateDirectories: true)
    defer { do { try FileManager.default.removeItem(at: directory) } catch { Issue.record(error) } }
    var info = [
      "CFBundleIdentifier": "com.apple.CompressorApp", "CFBundleName": "Compressor",
      "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "5.3",
      "CFBundleExecutable": "Compressor",
    ]
    if !includeVersion { info.removeValue(forKey: "CFBundleShortVersionString") }
    try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0).write(
      to: contents.appendingPathComponent("Info.plist"))
    var access = ApplicationAccess()
    access.locate = { $0 == "com.apple.CompressorApp" ? app : nil }
    access.running = { _ in false }
    access.open = { document, recipient in
      #expect(document.pathExtension == "compressor")
      #expect(recipient.path == app.path)
      return "com.apple.CompressorApp"
    }
    let inventory = Applications.inventory(access: access)
    #expect(inventory.count == 1)
    if includeVersion {
      #expect(inventory.first?.version == "5.3")
    } else {
      #expect(inventory.first?.version == "unknown")
    }
    #expect(try Applications.resolve(.compressor, bundleID: nil, access: access).path == app.path)
    #expect(throws: (any Error).self) {
      try Applications.resolve(.motion, bundleID: "com.apple.CompressorApp", access: access)
    }
    #expect(throws: (any Error).self) {
      try Applications.resolve(.motion, bundleID: nil, access: access)
    }
    let document = directory.appendingPathComponent("example.compressor")
    try Data().write(to: document)
    let result = try await Applications.openDocument(
      app: .compressor, bundleID: nil, path: document.path, access: access)
    #expect(result.contains("completion is not verified"))
    await #expect(throws: (any Error).self) {
      try await Applications.openDocument(
        app: .compressor, bundleID: nil,
        path: directory.appendingPathComponent("missing.compressor").path, access: access)
    }
    access.open = { _, _ in "wrong.recipient" }
    await #expect(throws: (any Error).self) {
      try await Applications.openDocument(
        app: .compressor, bundleID: nil, path: document.path, access: access)
    }
    #expect(throws: (any Error).self) { try Compressor.executable(bundleID: nil, access: access) }
    let executable = contents.appendingPathComponent("MacOS/Compressor")
    try FileManager.default.createDirectory(
      at: executable.deletingLastPathComponent(), withIntermediateDirectories: false)
    #expect(
      FileManager.default.createFile(
        atPath: executable.path, contents: Data(), attributes: [.posixPermissions: 0o700]))
    #expect(try Compressor.executable(bundleID: nil, access: access) == executable)
    access.locate = { _ in app }
    #expect(Applications.inventory(access: access).count == 1)
  }

  @Test(.timeLimit(.minutes(1)))
  func nativeOpenRejectsANonexistentRecipientWithoutLaunchingAnApp() async {
    let missing = FileManager.default.temporaryDirectory.appendingPathComponent(
      "missing-\(UUID().uuidString)")
    await #expect(throws: (any Error).self) {
      try await ApplicationAccess().open(
        missing.appendingPathExtension("fcpxml"), missing.appendingPathExtension("app"))
    }
    await #expect(throws: (any Error).self) {
      try await Applications.openDocument(
        app: .motion, bundleID: "not-an-approved-bundle", path: missing.path)
    }
  }

  @Test func nativeReadOnlyInventoryDoesNotLaunchApps() {
    // Read-only OS integration, no document delivery or launch. Missing apps
    // are an expected result on CI; no assumptions about installed software.
    let inventory = Applications.inventory()
    #expect(inventory.count <= 10)
    #expect(inventory.allSatisfy { $0.app.bundleIDs.contains($0.bundleID) })
  }
}
