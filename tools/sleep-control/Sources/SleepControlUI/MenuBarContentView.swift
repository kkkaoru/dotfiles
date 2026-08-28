import SleepControlCore
import SwiftUI

/// Provides sleep controls from the macOS menu bar.
@MainActor
public struct MenuBarContentView: View {
  private static let contentPadding: CGFloat = 14
  private static let contentSpacing: CGFloat = 12
  private static let contentWidth: CGFloat = 280

  @ObservedObject private var model: SleepSettingsModel
  private let shortcut: SleepToggleShortcut
  private let strings: MenuBarStrings
  private let openLegacySettings: @MainActor () -> Void
  private let quit: @MainActor () -> Void

  /// Builds a stable popover with status, toggle, settings, reload, and quit controls.
  public var body: some View {
    VStack(alignment: .leading, spacing: Self.contentSpacing) {
      Text(statusText)
        .font(.headline)
      toggleButton
      Divider()
      Button(strings.reload, action: model.refresh)
        .disabled(model.isBusy)
      settingsItem
      Divider()
      Button(strings.quit, action: quit)
    }
    .padding(Self.contentPadding)
    .frame(width: Self.contentWidth)
  }

  private var statusText: String {
    switch model.isSleepDisabled {
    case true:
      strings.disabledStatus

    case false:
      strings.enabledStatus

    case nil:
      strings.unavailableStatus
    }
  }

  private var toggleTitle: String {
    model.isSleepDisabled == true ? strings.enableSleep : strings.disableSleep
  }

  private var toggleButton: some View {
    HStack {
      Button(toggleTitle, action: model.toggleSleep)
        .disabled(model.isBusy || model.isSleepDisabled == nil)
      Spacer()
      Text(shortcut.displayName)
        .foregroundStyle(.secondary)
        .fixedSize()
    }
  }

  @ViewBuilder private var settingsItem: some View {
    if #available(macOS 14.0, *) {
      SettingsLink {
        Text(strings.settings)
      }
    } else {
      Button(strings.settings, action: openLegacySettings)
    }
  }

  /// Creates menu-bar content with app-owned settings and quit actions.
  public init(
    model: SleepSettingsModel,
    shortcut: SleepToggleShortcut,
    strings: MenuBarStrings = MenuBarStrings(),
    openSettings: @escaping @MainActor () -> Void,
    quit: @escaping @MainActor () -> Void
  ) {
    self.model = model
    self.shortcut = shortcut
    self.strings = strings
    openLegacySettings = openSettings
    self.quit = quit
  }
}
