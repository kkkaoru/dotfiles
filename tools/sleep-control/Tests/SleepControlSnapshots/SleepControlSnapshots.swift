import AppKit
import SleepControlCore
import SleepControlUI
import SwiftUI

@main
@MainActor
internal enum SleepControlSnapshots {
  private static let expectedArgumentCount = 3
  private static let resourcesArgumentIndex = 1
  private static let outputArgumentIndex = 2
  private static let expectedSnapshotCount = 6
  private static let languages = ["en", "ja"]
  private static let states = [
    (name: "sleep-enabled", value: false),
    (name: "sleep-disabled", value: true),
  ]

  internal static func main() throws {
    guard CommandLine.arguments.count == expectedArgumentCount else {
      throw SnapshotError.invalidArguments
    }

    let resources = URL(
      filePath: CommandLine.arguments[resourcesArgumentIndex],
      directoryHint: .isDirectory
    )
    let output = URL(
      filePath: CommandLine.arguments[outputArgumentIndex],
      directoryHint: .isDirectory
    )
    try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

    for language in languages {
      try render(language: language, resources: resources, output: output)
    }
    print("UI snapshots: \(expectedSnapshotCount) rendered")
  }

  private static func render(language: String, resources: URL, output: URL) throws {
    let localization = resources.appending(
      path: "\(language).lproj",
      directoryHint: .isDirectory
    )
    guard let bundle = Bundle(url: localization) else {
      throw SnapshotError.missingLocalization(language)
    }
    let strings = SleepControlStrings(bundle: bundle)

    for state in states {
      let model = SleepSettingsModel(client: SnapshotClient(sleepDisabled: state.value))
      model.refresh()
      let view = SleepControlView(model: model, strings: strings)
        .background(Color(nsColor: .windowBackgroundColor))
      let file = output.appending(
        path: "\(language)-\(state.name).png",
        directoryHint: .notDirectory
      )
      try write(view: view, to: file)
    }
    try renderSettings(language: language, bundle: bundle, output: output)
  }

  private static func renderSettings(language: String, bundle: Bundle, output: URL) throws {
    let settings = ShortcutSettingsStore(defaults: UserDefaults())
    settings.shortcut = .defaultValue
    let view = ShortcutSettingsView(
      settings: settings,
      isRegistered: true,
      strings: ShortcutSettingsStrings(bundle: bundle)
    ) { _ in
      // A static snapshot never changes the selected shortcut.
    }
    .background(Color(nsColor: .windowBackgroundColor))
    let file = output.appending(
      path: "\(language)-shortcut-settings.png",
      directoryHint: .notDirectory
    )
    try write(view: view, to: file)
  }

  private static func write<Content: View>(view: Content, to file: URL) throws {
    let hostingView = NSHostingView(rootView: view)
    hostingView.frame.size = hostingView.fittingSize
    hostingView.layoutSubtreeIfNeeded()

    guard
      hostingView.bounds.width > 0,
      hostingView.bounds.height > 0,
      let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds)
    else {
      throw SnapshotError.renderFailed
    }

    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
      throw SnapshotError.encodingFailed
    }
    try data.write(to: file, options: .atomic)
  }
}
