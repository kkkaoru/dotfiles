import Foundation

internal actor PowerCommandRecorder {
  internal private(set) var commands: [[String]] = []

  internal func run(_ executable: String, arguments: [String]) -> Data {
    commands.append([executable] + arguments)
    let record: String
    if arguments.contains("IOPMrootDomain") {
      record = "<key>AppleClamshellState</key><true/>"
    } else {
      record = """
        <key>BatteryInstalled</key><true/>
        <key>CurrentCapacity</key><integer>50</integer>
        <key>MaxCapacity</key><integer>100</integer>
        """
    }
    let document = """
      <?xml version="1.0"?><plist version="1.0">
      <array><dict>\(record)</dict></array></plist>
      """
    return Data(document.utf8)
  }
}
