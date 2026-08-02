#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import Foundation

// The literal shortcut keys mirror the private constants in ShortcutSettingsStore.
// testShortcutWritesRawValuesImmediately fails when they drift, which keeps the
// fallback tests below from passing vacuously against renamed keys.
extension SleepControlCoreTests {
  internal static func testShortcutPersistsEveryCombination() {
    let defaults = isolatedDefaults()
    let settings = ShortcutSettingsStore(defaults: defaults)

    for modifiers in ShortcutModifiers.allCases {
      for key in ShortcutKey.allCases {
        let shortcut = SleepToggleShortcut(modifiers: modifiers, key: key)
        settings.shortcut = shortcut
        expect(ShortcutSettingsStore(defaults: defaults).shortcut == shortcut)
      }
    }
  }

  internal static func testShortcutFallsBackWhenKeyIsMissing() {
    let defaults = isolatedDefaults()
    defaults.set("commandOption", forKey: "sleepToggleShortcut.modifiers")

    let expected = SleepToggleShortcut(modifiers: .commandOption, key: .letterS)
    expect(ShortcutSettingsStore(defaults: defaults).shortcut == expected)
  }

  internal static func testShortcutFallsBackWhenModifiersAreMissing() {
    let defaults = isolatedDefaults()
    defaults.set("m", forKey: "sleepToggleShortcut.key")

    let expected = SleepToggleShortcut(modifiers: .controlOption, key: .letterM)
    expect(ShortcutSettingsStore(defaults: defaults).shortcut == expected)
  }

  internal static func testShortcutFallsBackWhenValuesAreNotStrings() {
    let defaults = isolatedDefaults()
    defaults.set(["unexpected"], forKey: "sleepToggleShortcut.key")
    defaults.set(Data([0x01]), forKey: "sleepToggleShortcut.modifiers")

    expect(ShortcutSettingsStore(defaults: defaults).shortcut == .defaultValue)
  }

  internal static func testShortcutWritesRawValuesImmediately() {
    let defaults = isolatedDefaults()
    let settings = ShortcutSettingsStore(defaults: defaults)

    settings.shortcut = SleepToggleShortcut(modifiers: .controlShift, key: .letterK)

    expect(defaults.string(forKey: "sleepToggleShortcut.key") == "k")
    expect(defaults.string(forKey: "sleepToggleShortcut.modifiers") == "controlShift")
  }

  internal static func testBooleanSettingsFallBackToLegacyKeys() {
    let defaults = isolatedDefaults()
    defaults.set(false, forKey: "behavior.lidCloseDisplaySleep")
    defaults.set(false, forKey: "behavior.capsLockLight")

    let settings = ShortcutSettingsStore(defaults: defaults)

    expect(!settings.isLidDisplaySleepEnabled)
    expect(!settings.isCapsLockLightEnabled)
  }

  internal static func testBooleanSettingsPreferCurrentKeyOverLegacy() {
    let defaults = isolatedDefaults()
    defaults.set(true, forKey: ShortcutSettingsStore.lidDisplaySleepEnabledDefaultsKey)
    defaults.set(false, forKey: "behavior.lidCloseDisplaySleep")
    defaults.set(true, forKey: ShortcutSettingsStore.capsLockLightEnabledDefaultsKey)
    defaults.set(false, forKey: "behavior.capsLockLight")

    let settings = ShortcutSettingsStore(defaults: defaults)

    expect(settings.isLidDisplaySleepEnabled)
    expect(settings.isCapsLockLightEnabled)
  }

  internal static func testBooleanSettingsIgnoreNonBooleanLegacyValues() {
    let defaults = isolatedDefaults()
    defaults.set(["false"], forKey: "behavior.lidCloseDisplaySleep")
    defaults.set(["false"], forKey: "behavior.capsLockLight")

    let settings = ShortcutSettingsStore(defaults: defaults)

    expect(settings.isLidDisplaySleepEnabled)
    expect(settings.isCapsLockLightEnabled)
  }

  internal static func testBooleanSettingsWriteThroughRawValues() {
    let defaults = isolatedDefaults()
    let settings = ShortcutSettingsStore(defaults: defaults)

    settings.isLidDisplaySleepEnabled = false
    settings.isCapsLockLightEnabled = false

    let lidKey = ShortcutSettingsStore.lidDisplaySleepEnabledDefaultsKey
    let capsKey = ShortcutSettingsStore.capsLockLightEnabledDefaultsKey
    expect(defaults.object(forKey: lidKey) as? Bool == false)
    expect(defaults.object(forKey: capsKey) as? Bool == false)
  }

  internal static func testBooleanSettingsExposeBackwardCompatibleNames() {
    let defaults = isolatedDefaults()
    let settings = ShortcutSettingsStore(defaults: defaults)

    settings.sleepDisplaysWhenLidCloses = false
    settings.capsLockLightEnabled = false

    expect(!settings.isLidDisplaySleepEnabled)
    expect(!settings.isCapsLockLightEnabled)

    settings.isLidDisplaySleepEnabled = true
    settings.isCapsLockLightEnabled = true

    expect(settings.sleepDisplaysWhenLidCloses)
    expect(settings.capsLockLightEnabled)
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
