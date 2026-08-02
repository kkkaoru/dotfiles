#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import Foundation

extension SleepControlCoreTests {
  internal static func testUnifiedSettingsDefaults() {
    let settings = SleepControlSettingsStore(defaults: isolatedDefaults())

    expect(settings.isLidDisplaySleepEnabled)
    expect(settings.isCapsLockLightEnabled)
    expect(settings.shortcut == .defaultValue)
    expect(SleepControlSettingsStore.defaultsSuiteName == "com.kkkaoru.sleep-control")
    expect(
      SleepControlSettingsStore.lidDisplaySleepEnabledDefaultsKey
        == "lidDisplaySleepEnabled"
    )
  }

  internal static func testUnifiedSettingsPersistence() {
    let defaults = isolatedDefaults()
    let settings = SleepControlSettingsStore(defaults: defaults)
    settings.isLidDisplaySleepEnabled = false
    settings.isCapsLockLightEnabled = false
    settings.shortcut = SleepToggleShortcut(modifiers: .commandShift, key: .letterM)

    let restored = SleepControlSettingsStore(defaults: defaults)

    expect(!restored.isLidDisplaySleepEnabled)
    expect(!restored.isCapsLockLightEnabled)
    expect(restored.shortcut == settings.shortcut)
  }

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

  internal static func testShortcutInvalidPersistedValuesFallBackIndependently() {
    let defaults = isolatedDefaults()
    defaults.set(ShortcutModifiers.commandShift.rawValue, forKey: "sleepToggleShortcut.modifiers")
    defaults.set("not-a-key", forKey: "sleepToggleShortcut.key")

    let restored = ShortcutSettingsStore(defaults: defaults)

    expect(restored.shortcut.modifiers == .commandShift)
    expect(restored.shortcut.key == .letterS)

    defaults.set("not-a-modifier", forKey: "sleepToggleShortcut.modifiers")
    defaults.set(ShortcutKey.letterM.rawValue, forKey: "sleepToggleShortcut.key")
    let restoredWithInvalidModifiers = ShortcutSettingsStore(defaults: defaults)

    expect(restoredWithInvalidModifiers.shortcut.modifiers == .controlOption)
    expect(restoredWithInvalidModifiers.shortcut.key == .letterM)
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
