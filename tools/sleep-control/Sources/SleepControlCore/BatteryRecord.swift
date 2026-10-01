/// Typed read-only projection of the internal AppleSmartBattery registry entry.
internal struct BatteryRecord: Decodable {
  private enum CodingKeys: String, CodingKey {
    case current = "CurrentCapacity"
    case isInstalled = "BatteryInstalled"
    case maximum = "MaxCapacity"
  }

  private let isInstalled: Bool?
  private let current: Int?
  private let maximum: Int?

  internal var percentage: Double? {
    guard isInstalled == true, let current, let maximum, maximum > 0,
      (0...maximum).contains(current)
    else {
      return nil
    }
    return Double(current) / Double(maximum) * 100
  }
}
