import SleepControlCore
import SwiftUI

/// Lets the user configure Sleep Control's automatic controls and global shortcut.
@MainActor
public struct SleepControlSettingsView: View {
  private static let contentWidth: CGFloat = 440
  private static let contentPadding: CGFloat = 24
  private static let descriptionSpacing: CGFloat = 3

  @ObservedObject private var settings: SleepControlSettingsStore
  private let isRegistered: Bool
  private let strings: SleepControlSettingsStrings
  private let onShortcutChange: @MainActor (SleepToggleShortcut) -> Void

  /// Builds one settings form for display behavior, the indicator LED, and shortcut.
  public var body: some View {
    Form {
      automaticControlsSection
      shortcutSection
    }
    .formStyle(.grouped)
    .padding(Self.contentPadding)
    .frame(width: Self.contentWidth)
  }

  private var automaticControlsSection: some View {
    Section(strings.automaticControls) {
      settingToggle(
        strings.lidDisplaySleep,
        description: strings.lidDisplaySleepDescription,
        isOn: lidDisplaySleepEnabled
      )
      .accessibilityIdentifier("lid-display-sleep-toggle")
      settingToggle(
        strings.capsLockLight,
        description: strings.capsLockLightDescription,
        isOn: capsLockLightEnabled
      )
      .accessibilityIdentifier("caps-lock-light-toggle")
    }
  }

  private var shortcutSection: some View {
    Section(strings.shortcut) {
      shortcutPickers
      shortcutStatus
    }
  }

  private var shortcutPickers: some View {
    Group {
      Picker(strings.modifiers, selection: modifiers) {
        ForEach(ShortcutModifiers.allCases) { modifiers in
          Text(modifiers.displayName).tag(modifiers)
        }
      }
      .accessibilityIdentifier("shortcut-modifiers-picker")
      Picker(strings.key, selection: key) {
        ForEach(ShortcutKey.allCases) { key in
          Text(key.displayName).tag(key)
        }
      }
      .accessibilityIdentifier("shortcut-key-picker")
    }
  }

  @ViewBuilder private var shortcutStatus: some View {
    LabeledContent(strings.current, value: settings.shortcut.displayName)
      .accessibilityIdentifier("current-shortcut")
    if !isRegistered {
      Text(strings.conflict)
        .foregroundStyle(.red)
    }
    Text(strings.description)
      .font(.caption)
      .foregroundStyle(.secondary)
  }

  private var lidDisplaySleepEnabled: Binding<Bool> {
    Binding(
      get: { settings.isLidDisplaySleepEnabled },
      set: { settings.isLidDisplaySleepEnabled = $0 }
    )
  }

  private var capsLockLightEnabled: Binding<Bool> {
    Binding(
      get: { settings.isCapsLockLightEnabled },
      set: { settings.isCapsLockLightEnabled = $0 }
    )
  }

  private var modifiers: Binding<ShortcutModifiers> {
    Binding(
      get: { settings.shortcut.modifiers },
      set: { updateShortcut(modifiers: $0, key: settings.shortcut.key) }
    )
  }

  private var key: Binding<ShortcutKey> {
    Binding(
      get: { settings.shortcut.key },
      set: { updateShortcut(modifiers: settings.shortcut.modifiers, key: $0) }
    )
  }

  /// Creates the unified settings form and shortcut registration callback.
  public init(
    settings: SleepControlSettingsStore,
    isRegistered: Bool,
    strings: SleepControlSettingsStrings = SleepControlSettingsStrings(),
    onShortcutChange: @escaping @MainActor (SleepToggleShortcut) -> Void
  ) {
    self.settings = settings
    self.isRegistered = isRegistered
    self.strings = strings
    self.onShortcutChange = onShortcutChange
  }

  private func settingToggle(
    _ title: String,
    description: String,
    isOn: Binding<Bool>
  ) -> some View {
    Toggle(isOn: isOn) {
      VStack(alignment: .leading, spacing: Self.descriptionSpacing) {
        Text(title)
        Text(description)
          .font(.caption)
          .foregroundStyle(.secondary)
      }
    }
  }

  private func updateShortcut(modifiers: ShortcutModifiers, key: ShortcutKey) {
    let shortcut = SleepToggleShortcut(modifiers: modifiers, key: key)
    settings.shortcut = shortcut
    onShortcutChange(shortcut)
  }
}

/// Backward-compatible name for the former shortcut-only form.
public typealias ShortcutSettingsView = SleepControlSettingsView
