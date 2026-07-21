/// Extracts the system-wide `SleepDisabled` value from `pmset -g` output.
///
/// - Parameter output: Text emitted by `/usr/bin/pmset -g`.
/// - Returns: `true` for 1, `false` for 0, or `nil` when the value is unavailable.
public func parseSleepDisabled(from output: String) -> Bool? {
  let expectedFieldCount = 2
  for line in output.split(separator: "\n") {
    let fields = line.split(whereSeparator: \.isWhitespace)
    guard fields.first == "SleepDisabled", fields.count == expectedFieldCount else {
      continue
    }

    switch fields[1] {
    case "0":
      return false

    case "1":
      return true

    default:
      return nil
    }
  }
  return nil
}
