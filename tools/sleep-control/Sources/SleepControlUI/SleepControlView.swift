import SleepControlCore
import SwiftUI

/// Displays and edits the system-wide sleep-disabled setting.
@MainActor
public struct SleepControlView: View {
  private static let contentSpacing: CGFloat = 18
  private static let headerSpacing: CGFloat = 12
  private static let titleSpacing: CGFloat = 3
  private static let iconSize: CGFloat = 30
  private static let contentPadding: CGFloat = 24
  private static let contentWidth: CGFloat = 400

  @ObservedObject private var model: SleepSettingsModel
  private let strings: SleepControlStrings

  /// Composes the status, toggle, explanation, and reload controls.
  public var body: some View {
    VStack(alignment: .leading, spacing: Self.contentSpacing) {
      statusHeader
      sleepToggle
      explanation
      errorMessage
      controls
    }
    .padding(Self.contentPadding)
    .frame(width: Self.contentWidth)
    .onAppear(perform: model.refresh)
  }

  private var sleepEnabled: Binding<Bool> {
    Binding(
      get: { model.isSleepDisabled == false },
      set: { model.updateSleepDisabled(!$0) }
    )
  }

  private var statusHeader: some View {
    HStack(spacing: Self.headerSpacing) {
      Image(systemName: statusSymbolName)
        .font(.system(size: Self.iconSize))
        .foregroundStyle(statusColor)
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: Self.titleSpacing) {
        Text(strings.title)
          .font(.title2.bold())
        Text(statusText)
          .foregroundStyle(statusColor)
      }
    }
  }

  private var sleepToggle: some View {
    Toggle(strings.enableSleepToggle, isOn: sleepEnabled)
      .toggleStyle(.switch)
      .controlSize(.large)
      .disabled(model.isBusy || model.isSleepDisabled == nil)
  }

  private var explanation: some View {
    Text(strings.explanation)
      .font(.caption)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
  }

  @ViewBuilder private var errorMessage: some View {
    if let message = model.errorMessage {
      Text(message)
        .font(.caption)
        .foregroundStyle(.red)
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  private var controls: some View {
    HStack {
      if model.isBusy {
        ProgressView()
          .controlSize(.small)
      }
      Spacer()
      Button(strings.reloadButton, action: model.refresh)
        .disabled(model.isBusy)
    }
  }

  private var statusText: String {
    switch model.isSleepDisabled {
    case true:
      strings.disabledStatus

    case false:
      strings.enabledStatus

    case nil:
      model.isBusy ? strings.loadingStatus : strings.unavailableStatus
    }
  }

  private var statusColor: Color {
    model.isSleepDisabled == true ? .orange : .secondary
  }

  private var statusSymbolName: String {
    model.isSleepDisabled == true ? "sun.max.fill" : "moon.zzz.fill"
  }

  /// Creates a view backed by the provided model and localized text.
  public init(
    model: SleepSettingsModel,
    strings: SleepControlStrings = SleepControlStrings()
  ) {
    self.model = model
    self.strings = strings
  }
}
