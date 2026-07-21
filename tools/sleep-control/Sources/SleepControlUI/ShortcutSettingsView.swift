import SleepControlCore
import SwiftUI

/// Lets the user configure the global sleep-toggle shortcut.
@MainActor
public struct ShortcutSettingsView: View {
  private static let contentWidth: CGFloat = 360
  private static let contentPadding: CGFloat = 24

  @ObservedObject private var settings: ShortcutSettingsStore
  private let isRegistered: Bool
  private let strings: ShortcutSettingsStrings
  private let onShortcutChange: @MainActor (SleepToggleShortcut) -> Void

  /// Builds modifier and key pickers with registration feedback.
  public var body: some View {
    Form {
      Picker(strings.modifiers, selection: modifiers) {
        ForEach(ShortcutModifiers.allCases) { modifiers in
          Text(modifiers.displayName).tag(modifiers)
        }
      }
      Picker(strings.key, selection: key) {
        ForEach(ShortcutKey.allCases) { key in
          Text(key.displayName).tag(key)
        }
      }
      LabeledContent(strings.current, value: settings.shortcut.displayName)
      if !isRegistered {
        Text(strings.conflict)
          .foregroundStyle(.red)
      }
      Text(strings.description)
        .font(.caption)
        .foregroundStyle(.secondary)
    }
    .formStyle(.grouped)
    .padding(Self.contentPadding)
    .frame(width: Self.contentWidth)
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

  /// Creates the shortcut settings form and registration callback.
  public init(
    settings: ShortcutSettingsStore,
    isRegistered: Bool,
    strings: ShortcutSettingsStrings = ShortcutSettingsStrings(),
    onShortcutChange: @escaping @MainActor (SleepToggleShortcut) -> Void
  ) {
    self.settings = settings
    self.isRegistered = isRegistered
    self.strings = strings
    self.onShortcutChange = onShortcutChange
  }

  private func updateShortcut(modifiers: ShortcutModifiers, key: ShortcutKey) {
    let shortcut = SleepToggleShortcut(modifiers: modifiers, key: key)
    settings.shortcut = shortcut
    onShortcutChange(shortcut)
  }
}
