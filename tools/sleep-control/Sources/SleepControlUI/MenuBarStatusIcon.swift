import SleepControlCore
import SwiftUI

/// Displays the current sleep state as a compact menu-bar symbol.
@MainActor
public struct MenuBarStatusIcon: View {
  @ObservedObject private var model: SleepSettingsModel

  /// Shows a moon for enabled sleep and a sun for disabled sleep.
  public var body: some View {
    Image(systemName: symbolName)
      .accessibilityLabel(accessibilityLabel)
  }

  private var symbolName: String {
    switch model.isSleepDisabled {
    case true:
      "sun.max.fill"

    case false:
      "moon.fill"

    case nil:
      "questionmark.circle"
    }
  }

  private var accessibilityLabel: LocalizedStringKey {
    switch model.isSleepDisabled {
    case true:
      "status.disabled"

    case false:
      "status.enabled"

    case nil:
      "status.unavailable"
    }
  }

  /// Creates a status icon backed by the shared sleep model.
  public init(model: SleepSettingsModel) {
    self.model = model
  }
}
