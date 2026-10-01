import AppKit
import SleepControlCore
import SleepControlUI
import SwiftUI

extension SleepControlSnapshots {
  internal static func testSettingsObservation() throws {
    let settings = ShortcutSettingsStore(defaults: UserDefaults())
    settings.shortcut = .defaultValue
    var observed: [SleepToggleShortcut] = []
    let view = SleepControlSettingsView(settings: settings, isRegistered: true) { shortcut in
      observed.append(shortcut)
    }
    let host = NSHostingView(rootView: view)
    host.frame.size = host.fittingSize
    host.layoutSubtreeIfNeeded()
    settings.shortcut.key = .letterA
    let expected = SleepToggleShortcut(modifiers: .controlOption, key: .letterA)
    guard observed == [expected] else { throw SnapshotError.settingsObservationFailed }
    settings.batterySleep.isEnabled = false
    guard observed == [expected] else { throw SnapshotError.settingsObservationFailed }
    withExtendedLifetime(host) {
      // Keep the subscription alive until both changes have been observed.
    }
  }
}
