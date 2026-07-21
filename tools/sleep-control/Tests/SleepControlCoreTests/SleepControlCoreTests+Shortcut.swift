#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import Foundation

extension SleepControlCoreTests {
  internal static func testShortcutDefaults() {
    let settings = ShortcutSettingsStore(defaults: isolatedDefaults())

    expect(settings.shortcut == .defaultValue)
    expect(settings.shortcut.displayName == "⌃⌥S")
  }

  internal static func testShortcutPersistence() {
    let defaults = isolatedDefaults()
    let settings = ShortcutSettingsStore(defaults: defaults)
    settings.shortcut = SleepToggleShortcut(modifiers: .commandShift, key: .letterM)

    let restored = ShortcutSettingsStore(defaults: defaults)

    expect(restored.shortcut == settings.shortcut)
    expect(restored.shortcut.displayName == "⌘⇧M")
  }

  internal static func testShortcutPresentation() {
    let keys = ShortcutKey.allCases
    expect(keys.map(\.id) == keys.map(\.rawValue))
    expect(keys.map(\.displayName) == keys.map { $0.rawValue.uppercased() })

    let modifiers = ShortcutModifiers.allCases
    expect(modifiers.map(\.id) == modifiers.map(\.rawValue))
    expect(
      modifiers.map(\.displayName) == ["⌘⌥", "⌘⇧", "⌃⌘", "⌃⌥", "⌃⌥⌘", "⌃⇧"]
    )
  }

  private static func isolatedDefaults() -> UserDefaults {
    let name = "SleepControlCoreTests.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: name) else {
      preconditionFailure("Could not create isolated user defaults")
    }
    defaults.removePersistentDomain(forName: name)
    return defaults
  }
}
