#if canImport(SleepControlCore)
  import SleepControlCore
#endif

// The menu bar shows SleepToggleShortcut.displayName while the settings window
// shows the separate picker labels, so the composed name must always equal the
// labels the user selected.
extension SleepControlCoreTests {
  internal static func testShortcutDisplayNamesComposePickerLabels() {
    for modifiers in ShortcutModifiers.allCases {
      for key in ShortcutKey.allCases {
        let shortcut = SleepToggleShortcut(modifiers: modifiers, key: key)
        expect(shortcut.displayName == modifiers.displayName + key.displayName)
      }
    }
  }

  internal static func testKeyDisplayNamesCoverAlphabet() {
    let expected = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".map(String.init)
    let names = ShortcutKey.allCases.map(\.displayName)

    expect(names == expected)
    expect(names.allSatisfy { $0.count == 1 })
  }

  internal static func testModifierDisplayNamesAreDistinctGlyphs() {
    let names = ShortcutModifiers.allCases.map(\.displayName)
    let allowedGlyphs = Set("⌘⌥⌃⇧".map(String.init))

    expect(Set(names).count == names.count)
    expect(names.allSatisfy { !$0.isEmpty })
    expect(
      names.allSatisfy { name in
        Set(name.map(String.init)).isSubset(of: allowedGlyphs)
      }
    )
  }

  internal static func testDefaultShortcutIsComposed() {
    expect(
      SleepToggleShortcut.defaultValue
        == SleepToggleShortcut(modifiers: .controlOption, key: .letterS)
    )
  }
}
