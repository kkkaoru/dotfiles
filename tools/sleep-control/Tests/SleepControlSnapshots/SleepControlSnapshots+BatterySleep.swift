import AppKit
import SleepControlCore
import SleepControlUI
import SwiftUI

extension SleepControlSnapshots {
  internal static func renderBatterySleepError(
    language: String, bundle: Bundle, output: URL
  ) throws {
    let settings = ShortcutSettingsStore(defaults: UserDefaults())
    settings.shortcut = .defaultValue
    settings.batterySleep.isEnabled = false
    settings.batterySleep.threshold = .percent90
    let message = bundle.localizedString(
      forKey: "error.authorization_unavailable", value: nil, table: nil
    )
    let view = SleepControlSettingsView(
      settings: settings,
      isRegistered: false,
      strings: SleepControlSettingsStrings(bundle: bundle),
      onShortcutChange: { _ in
        // Snapshot rendering never registers a live global shortcut.
      },
      batterySleepError: message
    )
    .background(Color(nsColor: .windowBackgroundColor))
    let file = output.appending(path: "\(language)-battery-sleep-off.png")
    try write(view: view, to: file)
  }
}
