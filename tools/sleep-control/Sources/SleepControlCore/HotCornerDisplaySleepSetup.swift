import CoreGraphics
import Foundation

/// The Dock's hot corners assigned to "Put Display to Sleep".
public struct HotCornerDisplaySleepSetup: Sendable {
  /// Preferences suite in which the Dock stores hot corners.
  public static let dockDefaultsSuiteName = "com.apple.dock"

  private static let displaySleepAction = 10
  // Hot corners fire with the pointer against the screen edge, so a few points are enough.
  private static let cursorTolerance: CGFloat = 8

  private let requiredModifiers: [HotCorner: UInt]

  /// Reads the current hot-corner assignments from the Dock's preferences.
  public init(defaults: UserDefaults) {
    var modifiers: [HotCorner: UInt] = [:]
    for corner in HotCorner.allCases
    where defaults.integer(forKey: "wvous-\(corner.rawValue)-corner") == Self.displaySleepAction {
      modifiers[corner] = UInt(
        bitPattern: defaults.integer(forKey: "wvous-\(corner.rawValue)-modifier")
      )
    }
    requiredModifiers = modifiers
  }

  /// Whether the cursor and held modifiers match an assigned corner of any screen.
  ///
  /// The cursor and screens use AppKit's global coordinates, with the origin at the bottom left.
  public func isTriggered(cursor: CGPoint, screens: [CGRect], modifiers: UInt) -> Bool {
    requiredModifiers.contains { corner, required in
      modifiers & required == required
        && screens.contains { screen in
          let point = corner.point(of: screen)
          return abs(cursor.x - point.x) <= Self.cursorTolerance
            && abs(cursor.y - point.y) <= Self.cursorTolerance
        }
    }
  }
}
