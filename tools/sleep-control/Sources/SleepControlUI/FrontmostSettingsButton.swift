import AppKit
import SwiftUI

/// Opens Settings in front of other apps; `SettingsLink` alone leaves a menu-bar app behind them.
@available(macOS 14.0, *)
@MainActor
internal struct FrontmostSettingsButton: View {
  @Environment(\.openSettings)
  private var openSettings

  internal let title: String

  internal var body: some View {
    Button(title) {
      openSettings()
      NSApp.activate()
    }
  }
}
