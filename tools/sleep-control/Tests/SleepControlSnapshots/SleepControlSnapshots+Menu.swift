import AppKit
import SleepControlCore
import SleepControlUI
import SwiftUI

extension SleepControlSnapshots {
  internal static func renderMenu(
    language: String,
    stateName: String,
    model: SleepSettingsModel,
    strings: MenuBarStrings,
    output: URL
  ) throws {
    let view = MenuBarContentView(
      model: model,
      shortcut: .defaultValue,
      strings: strings,
      openSettings: {
        // A static snapshot never opens Settings.
      },
      quit: {
        // A static snapshot never terminates the process.
      }
    )
    .background(Color(nsColor: .windowBackgroundColor))
    let file = output.appending(
      path: "\(language)-menu-\(stateName).png",
      directoryHint: .notDirectory
    )
    try write(view: view, to: file)
  }
}
