import AppKit
import Foundation
import Testing

@testable import ProAppsCore

/// Validates generated effect carriers against the installed Final Cut Pro DTD.
/// Only the app's DTD files are read; Final Cut Pro is not launched.
struct FinalCutCarrierDTDTests {
  @Test func generatedCarriersAreValidAgainstTheInstalledDTD() async throws {
    let application = try #require(
      NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.FinalCutApp"))
    let dtd = application.appendingPathComponent(
      "Contents/Frameworks/Interchange.framework/Versions/A/Resources")
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "carrier-dtd-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
    defer { do { try FileManager.default.removeItem(at: root) } catch { Issue.record(error) } }
    let keyframes = FCPAnimatedValue(keyframes: [
      FCPKeyframe(seconds: 1, value: "0 0"), FCPKeyframe(seconds: 2, value: "50 0", curve: .smooth),
    ])
    let spec = FCPCarrierSpec(
      durationSeconds: 90,
      opacity: FCPAnimatedValue(keyframes: [
        FCPKeyframe(seconds: 60, value: "1"), FCPKeyframe(seconds: 70, value: "0"),
      ]), position: keyframes, scale: FCPAnimatedValue(value: "1.2 1.2"),
      rotation: FCPAnimatedValue(value: "10"), anchor: FCPAnimatedValue(value: "0 0"),
      effects: [
        FCPEffectSpec(
          uid: ".../Effects.localized/Blur.localized/Gaussian.localized/Gaussian.moef",
          name: "Gaussian",
          parameters: [
            FCPEffectParameter(
              name: "Amount", key: "9999/986883370/100/986883376/2/100",
              animation: FCPAnimatedValue(keyframes: [
                FCPKeyframe(seconds: 0, value: "0"), FCPKeyframe(seconds: 4, value: "1"),
              ]))
          ])
      ])
    let document = try FCPCarrierDocument.make(spec, workDirectory: root, project: "carrier-dtd")
    let file = root.appendingPathComponent("carrier.fcpxml")
    try Data(document.xml.utf8).write(to: file)
    let validation = try await FCPXMLValidator(dtdDirectory: dtd, workspace: root).validate(
      path: file.path)
    #expect(validation.validDTD)
    #expect(validation.version == "1.13")
  }
}
