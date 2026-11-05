#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import CoreGraphics
import Foundation

extension SleepControlCoreTests {
  private static let tolerance: CGFloat = 8
  private static let outsideTolerance: CGFloat = 9
  private static let mainWidth: CGFloat = 3_840
  private static let mainHeight: CGFloat = 2_160
  private static let sideWidth: CGFloat = 1_680
  private static let sideHeight: CGFloat = 1_050
  private static let sideY: CGFloat = 945
  private static let mainScreen = CGRect(x: 0, y: 0, width: mainWidth, height: mainHeight)
  private static let sideScreen = CGRect(
    x: -sideWidth,
    y: sideY,
    width: sideWidth,
    height: sideHeight
  )
  private static let screens = [mainScreen, sideScreen]
  private static let displaySleepAction = 10
  private static let otherAction = 1
  private static let commandModifier: UInt = 1_048_576
  private static let shiftModifier: UInt = 131_072

  internal static func testHotCornerSetupMatchesAssignedCornerOnAnyScreen() {
    let defaults = hotCornerDefaults()
    defaults.set(displaySleepAction, forKey: "wvous-bl-corner")
    defaults.set(0, forKey: "wvous-bl-modifier")
    defaults.set(otherAction, forKey: "wvous-br-corner")
    let setup = HotCornerDisplaySleepSetup(defaults: defaults)
    let tolerated = CGPoint(x: tolerance, y: tolerance)
    let outside = CGPoint(x: outsideTolerance, y: 0)
    let unassigned = CGPoint(x: mainScreen.maxX, y: 0)
    let topLeft = CGPoint(x: 0, y: mainScreen.maxY)

    expect(setup.isTriggered(cursor: .zero, screens: screens, modifiers: 0))
    expect(setup.isTriggered(cursor: tolerated, screens: screens, modifiers: 0))
    expect(setup.isTriggered(cursor: sideScreen.origin, screens: screens, modifiers: 0))
    expect(!setup.isTriggered(cursor: outside, screens: screens, modifiers: 0))
    expect(!setup.isTriggered(cursor: unassigned, screens: screens, modifiers: 0))
    expect(!setup.isTriggered(cursor: topLeft, screens: screens, modifiers: 0))
    expect(!setup.isTriggered(cursor: .zero, screens: [], modifiers: 0))
  }

  internal static func testHotCornerSetupCoversEveryCorner() {
    let screen = mainScreen
    let cases: [(key: String, cursor: CGPoint)] = [
      ("wvous-tl-corner", CGPoint(x: screen.minX, y: screen.maxY)),
      ("wvous-tr-corner", CGPoint(x: screen.maxX, y: screen.maxY)),
      ("wvous-bl-corner", CGPoint(x: screen.minX, y: screen.minY)),
      ("wvous-br-corner", CGPoint(x: screen.maxX, y: screen.minY)),
    ]

    for (key, cursor) in cases {
      let defaults = hotCornerDefaults()
      defaults.set(displaySleepAction, forKey: key)
      let setup = HotCornerDisplaySleepSetup(defaults: defaults)
      expect(setup.isTriggered(cursor: cursor, screens: [screen], modifiers: 0))
    }
  }

  internal static func testHotCornerSetupRequiresTheAssignedModifier() {
    let defaults = hotCornerDefaults()
    defaults.set(displaySleepAction, forKey: "wvous-tr-corner")
    defaults.set(Int(commandModifier), forKey: "wvous-tr-modifier")
    let setup = HotCornerDisplaySleepSetup(defaults: defaults)
    let cursor = CGPoint(x: mainScreen.maxX, y: mainScreen.maxY)

    expect(!setup.isTriggered(cursor: cursor, screens: screens, modifiers: 0))
    expect(setup.isTriggered(cursor: cursor, screens: screens, modifiers: commandModifier))
    let both = commandModifier | shiftModifier
    expect(setup.isTriggered(cursor: cursor, screens: screens, modifiers: both))
  }

  internal static func testHotCornerSetupWithoutAssignmentNeverMatches() {
    let setup = HotCornerDisplaySleepSetup(defaults: hotCornerDefaults())

    expect(!setup.isTriggered(cursor: .zero, screens: [mainScreen], modifiers: 0))
  }

  internal static func testHotCornerHoldPreferenceDefaultsOnAndPersists() {
    let defaults = hotCornerDefaults()
    let settings = ShortcutSettingsStore(defaults: defaults)

    expect(settings.isHotCornerDisplaySleepHoldEnabled)
    settings.isHotCornerDisplaySleepHoldEnabled = false

    let key = ShortcutSettingsStore.hotCornerHoldEnabledDefaultsKey
    expect(defaults.object(forKey: key) as? Bool == false)
    expect(!ShortcutSettingsStore(defaults: defaults).isHotCornerDisplaySleepHoldEnabled)
  }

  private static func hotCornerDefaults() -> UserDefaults {
    let name = "SleepControlCoreTests.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: name) else {
      preconditionFailure("Could not create isolated user defaults")
    }
    defaults.removePersistentDomain(forName: name)
    return defaults
  }
}
