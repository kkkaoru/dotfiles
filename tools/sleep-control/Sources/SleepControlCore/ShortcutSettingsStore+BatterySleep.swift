extension ShortcutSettingsStore {
  /// SwiftUI slider binding; rejects values outside the exact 0–90%, 10%-step choices.
  public var batterySleepThresholdPercentage: Double {
    get { Double(batterySleep.threshold.rawValue) }
    set {
      guard
        let threshold = BatterySleepThreshold.allCases.first(where: { choice in
          Double(choice.rawValue) == newValue
        })
      else {
        return
      }
      batterySleep.threshold = threshold
    }
  }
}
