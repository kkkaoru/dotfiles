import SleepControlCore
import SwiftUI

/// Provides sleep controls from the macOS menu bar.
@MainActor
public struct MenuBarContentView: View {
  @ObservedObject private var model: SleepSettingsModel
  private let shortcut: SleepToggleShortcut
  private let openLegacySettings: @MainActor () -> Void
  private let quit: @MainActor () -> Void

  /// Builds status, toggle, settings, reload, and quit menu items.
  public var body: some View {
    Text(statusText)
    Button(toggleTitle, action: model.toggleSleep)
      .disabled(model.isBusy || model.isSleepDisabled == nil)
    Text(shortcut.displayName)
    Divider()
    Button("button.reload", action: model.refresh)
      .disabled(model.isBusy)
    settingsItem
    Divider()
    Button("menu.quit", action: quit)
  }

  private var statusText: LocalizedStringKey {
    model.isSleepDisabled == true ? "status.disabled" : "status.enabled"
  }

  private var toggleTitle: LocalizedStringKey {
    model.isSleepDisabled == true ? "menu.enable_sleep" : "menu.disable_sleep"
  }

  @ViewBuilder private var settingsItem: some View {
    if #available(macOS 14.0, *) {
      SettingsLink {
        Text("menu.settings")
      }
    } else {
      Button("menu.settings", action: openLegacySettings)
    }
  }

  /// Creates menu content with app-owned settings and quit actions.
  public init(
    model: SleepSettingsModel,
    shortcut: SleepToggleShortcut,
    openSettings: @escaping @MainActor () -> Void,
    quit: @escaping @MainActor () -> Void
  ) {
    self.model = model
    self.shortcut = shortcut
    openLegacySettings = openSettings
    self.quit = quit
  }
}
