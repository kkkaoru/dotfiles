import Foundation

/// A fail-closed snapshot of the laptop lid and internal battery, independent of AC connection.
public struct BatterySleepReading: Sendable {
  /// Unknown lid state never authorizes sleep.
  public let lidIsClosed: Bool?
  /// Unknown or invalid battery capacity never authorizes sleep.
  public let batteryPercentage: Double?

  /// Creates a sensor snapshot; out-of-range percentages become unavailable.
  public init(lidIsClosed: Bool?, batteryPercentage: Double?) {
    self.lidIsClosed = lidIsClosed
    self.batteryPercentage = batteryPercentage.flatMap { (0...100).contains($0) ? $0 : nil }
  }

  /// Decodes the public ioreg plist output for IOPMrootDomain and AppleSmartBattery.
  public init(lidData: Data, batteryData: Data) throws {
    let decoder = PropertyListDecoder()
    let lids = try decoder.decode([LidRecord].self, from: lidData)
    let batteries = try decoder.decode([BatteryRecord].self, from: batteryData)
    self.init(lidIsClosed: lids.first?.isClosed, batteryPercentage: batteries.first?.percentage)
  }

  /// Tests the inclusive cutoff without treating missing sensors as a low battery.
  public func requiresSleep(settings: BatterySleepSettings) -> Bool {
    guard settings.isEnabled, lidIsClosed == true, let batteryPercentage else {
      return false
    }
    return batteryPercentage <= Double(settings.threshold.rawValue)
  }
}
