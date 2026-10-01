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
  private let batterySleepError: String?

  /// Builds one settings form for display behavior, the indicator LED, and shortcut.
  public var body: some View {
    Form {
      automaticControlsSection
      BatterySleepSettingsView(
        settings: settings, strings: strings, errorMessage: batterySleepError
      )
      shortcutSection
    }
    .formStyle(.grouped)
    .padding(Self.contentPadding)
    .frame(width: Self.contentWidth)
    .onReceive(settings.$shortcut.dropFirst(), perform: onShortcutChange)
  }

  private var automaticControlsSection: some View {
    Section(strings.automaticControls) {
      settingToggle(
        strings.lidDisplaySleep,
        description: strings.lidDisplaySleepDescription,
        isOn: $settings.isLidDisplaySleepEnabled
      )
      .accessibilityIdentifier("lid-display-sleep-toggle")
      settingToggle(
        strings.capsLockLight,
        description: strings.capsLockLightDescription,
        isOn: $settings.isCapsLockLightEnabled
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
      Picker(strings.modifiers, selection: $settings.shortcut.modifiers) {
        ForEach(ShortcutModifiers.allCases) { modifiers in
          Text(modifiers.displayName).tag(modifiers)
        }
      }
      .accessibilityIdentifier("shortcut-modifiers-picker")
      Picker(strings.key, selection: $settings.shortcut.key) {
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

  /// Creates the unified settings form and shortcut registration callback.
  public init(
    settings: SleepControlSettingsStore,
    isRegistered: Bool,
    strings: SleepControlSettingsStrings = SleepControlSettingsStrings(),
    onShortcutChange: @escaping @MainActor (SleepToggleShortcut) -> Void,
    batterySleepError: String? = nil
  ) {
    self.settings = settings
    self.isRegistered = isRegistered
    self.strings = strings
    self.onShortcutChange = onShortcutChange
    self.batterySleepError = batterySleepError
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
}

/// Backward-compatible name for the former shortcut-only form.
public typealias ShortcutSettingsView = SleepControlSettingsView
