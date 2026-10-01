import SleepControlCore
import SwiftUI

/// Independent persisted controls for closed-lid low-battery sleep.
@MainActor
internal struct BatterySleepSettingsView: View {
  private static let maximumPercentage = 90.0
  private static let stepPercentage = 10.0

  @ObservedObject internal var settings: SleepControlSettingsStore
  internal let strings: SleepControlSettingsStrings
  internal let errorMessage: String?

  internal var body: some View {
    Section(strings.batterySleep) {
      Toggle(strings.batterySleepEnabled, isOn: $settings.batterySleep.isEnabled)
        .accessibilityIdentifier("battery-sleep-toggle")
      LabeledContent(
        strings.batterySleepThreshold, value: "\(settings.batterySleep.threshold.rawValue)%"
      )
      thresholdSlider
      Text(strings.batterySleepDescription)
        .font(.caption)
        .foregroundStyle(.secondary)
      if let errorMessage {
        Text(errorMessage)
          .foregroundStyle(.red)
          .accessibilityIdentifier("battery-sleep-error")
      }
    }
  }

  private var thresholdSlider: some View {
    Slider(
      value: $settings.batterySleepThresholdPercentage,
      in: 0...Self.maximumPercentage,
      step: Self.stepPercentage
    ) {
      Text(strings.batterySleepThreshold)
    } minimumValueLabel: {
      Text("0%")
    } maximumValueLabel: {
      Text("90%")
    }
    .labelsHidden()
    .disabled(!settings.batterySleep.isEnabled)
    .accessibilityIdentifier("battery-sleep-threshold-slider")
    .accessibilityValue("\(settings.batterySleep.threshold.rawValue)%")
  }
}
