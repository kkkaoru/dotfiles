import Carbon.HIToolbox
import SleepControlCore

extension ShortcutModifiers {
  private static let carbonFlagsByModifiers: [Self: UInt32] = [
    .commandOption: UInt32(cmdKey | optionKey),
    .commandShift: UInt32(cmdKey | shiftKey),
    .controlCommand: UInt32(controlKey | cmdKey),
    .controlOption: UInt32(controlKey | optionKey),
    .controlOptionCommand: UInt32(controlKey | optionKey | cmdKey),
    .controlShift: UInt32(controlKey | shiftKey),
  ]

  internal var carbonFlags: UInt32 {
    Self.carbonFlagsByModifiers[self] ?? UInt32(controlKey | optionKey)
  }
}
