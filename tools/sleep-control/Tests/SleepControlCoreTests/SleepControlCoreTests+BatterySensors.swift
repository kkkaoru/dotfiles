#if canImport(SleepControlCore)
  import SleepControlCore
#endif

import Foundation

extension SleepControlCoreTests {
  internal static func testBatterySensorPlists() throws {
    let expectedPercentage = 50.0
    let closed = sensorPlist("<dict><key>AppleClamshellState</key><true/></dict>")
    let reading = try BatterySleepReading(
      lidData: closed,
      batteryData: sensorPlist(
        """
        <dict><key>BatteryInstalled</key><true/>
        <key>CurrentCapacity</key><integer>3000</integer>
        <key>MaxCapacity</key><integer>6000</integer></dict>
        """
      )
    )
    expect(reading.lidIsClosed == true)
    expect(reading.batteryPercentage == expectedPercentage)
    let empty = try BatterySleepReading(lidData: sensorPlist(""), batteryData: sensorPlist(""))
    expect(empty.batteryPercentage == nil)
    let missing = try BatterySleepReading(
      lidData: sensorPlist("<dict/>"), batteryData: sensorPlist("<dict/>")
    )
    expect(missing.lidIsClosed == nil)
    expect(missing.batteryPercentage == nil)
    try testInvalidBatteryCapacities(lidData: closed)
    do {
      _ = try BatterySleepReading(lidData: Data("not a plist".utf8), batteryData: sensorPlist(""))
      throw TestError.readFailed
    } catch is DecodingError {
      // Malformed sensor output must be an error, never a zero-percent reading.
    }
  }

  private static func testInvalidBatteryCapacities(lidData: Data) throws {
    let zeroMaximum = try BatterySleepReading(
      lidData: lidData,
      batteryData: sensorPlist(
        """
        <dict><key>BatteryInstalled</key><true/>
        <key>CurrentCapacity</key><integer>0</integer>
        <key>MaxCapacity</key><integer>0</integer></dict>
        """
      )
    )
    expect(zeroMaximum.batteryPercentage == nil)
    let excessive = try BatterySleepReading(
      lidData: lidData,
      batteryData: sensorPlist(
        """
        <dict><key>BatteryInstalled</key><true/>
        <key>CurrentCapacity</key><integer>101</integer>
        <key>MaxCapacity</key><integer>100</integer></dict>
        """
      )
    )
    expect(excessive.batteryPercentage == nil)
    let absent = try BatterySleepReading(
      lidData: lidData,
      batteryData: sensorPlist("<dict><key>BatteryInstalled</key><false/></dict>")
    )
    expect(absent.batteryPercentage == nil)
  }

  private static func sensorPlist(_ records: String) -> Data {
    Data("<?xml version=\"1.0\"?><plist version=\"1.0\"><array>\(records)</array></plist>".utf8)
  }
}
