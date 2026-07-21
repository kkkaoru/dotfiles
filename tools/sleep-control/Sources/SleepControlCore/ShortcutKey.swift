/// A letter key available for the global sleep-toggle shortcut.
public enum ShortcutKey: String, CaseIterable, Identifiable, Sendable {
  case letterA = "a"
  case letterB = "b"
  case letterC = "c"
  case letterD = "d"
  case letterE = "e"
  case letterF = "f"
  case letterG = "g"
  case letterH = "h"
  case letterI = "i"
  case letterJ = "j"
  case letterK = "k"
  case letterL = "l"
  case letterM = "m"
  case letterN = "n"
  case letterO = "o"
  case letterP = "p"
  case letterQ = "q"
  case letterR = "r"
  case letterS = "s"
  case letterT = "t"
  case letterU = "u"
  case letterV = "v"
  case letterW = "w"
  case letterX = "x"
  case letterY = "y"
  case letterZ = "z"

  /// Stable identity used by SwiftUI pickers.
  public var id: String {
    rawValue
  }

  /// Uppercase key name shown to the user.
  public var displayName: String {
    rawValue.uppercased()
  }
}
