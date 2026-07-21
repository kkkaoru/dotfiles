import Carbon.HIToolbox
import SleepControlCore

extension ShortcutKey {
  private static let carbonKeyCodes: [Self: UInt32] = [
    .letterA: UInt32(kVK_ANSI_A),
    .letterB: UInt32(kVK_ANSI_B),
    .letterC: UInt32(kVK_ANSI_C),
    .letterD: UInt32(kVK_ANSI_D),
    .letterE: UInt32(kVK_ANSI_E),
    .letterF: UInt32(kVK_ANSI_F),
    .letterG: UInt32(kVK_ANSI_G),
    .letterH: UInt32(kVK_ANSI_H),
    .letterI: UInt32(kVK_ANSI_I),
    .letterJ: UInt32(kVK_ANSI_J),
    .letterK: UInt32(kVK_ANSI_K),
    .letterL: UInt32(kVK_ANSI_L),
    .letterM: UInt32(kVK_ANSI_M),
    .letterN: UInt32(kVK_ANSI_N),
    .letterO: UInt32(kVK_ANSI_O),
    .letterP: UInt32(kVK_ANSI_P),
    .letterQ: UInt32(kVK_ANSI_Q),
    .letterR: UInt32(kVK_ANSI_R),
    .letterS: UInt32(kVK_ANSI_S),
    .letterT: UInt32(kVK_ANSI_T),
    .letterU: UInt32(kVK_ANSI_U),
    .letterV: UInt32(kVK_ANSI_V),
    .letterW: UInt32(kVK_ANSI_W),
    .letterX: UInt32(kVK_ANSI_X),
    .letterY: UInt32(kVK_ANSI_Y),
    .letterZ: UInt32(kVK_ANSI_Z),
  ]

  internal var carbonKeyCode: UInt32 {
    Self.carbonKeyCodes[self] ?? UInt32(kVK_ANSI_S)
  }
}
