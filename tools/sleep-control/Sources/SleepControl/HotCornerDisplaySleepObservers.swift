import AppKit
import SleepControlCore
import SwiftUI

/// Feeds the preference and native display, sleep and lid events to the controller.
internal struct HotCornerDisplaySleepObservers: ViewModifier {
  @ObservedObject internal var settings: ShortcutSettingsStore
  internal let controller: HotCornerDisplaySleepController

  internal func body(content: Content) -> some View {
    content
      .onReceive(settings.$isHotCornerDisplaySleepHoldEnabled) { enabled in
        controller.setEnabled(enabled)
      }
      .onReceive(workspaceNotifications(NSWorkspace.screensDidSleepNotification)) { _ in
        controller.displaysDidSleep()
      }
      .onReceive(workspaceNotifications(NSWorkspace.screensDidWakeNotification)) { _ in
        controller.displaysDidWake()
      }
      .onReceive(workspaceNotifications(NSWorkspace.willSleepNotification)) { _ in
        controller.endHold()
      }
      .onReceive(NotificationCenter.default.publisher(for: BatterySleepEvents.lidChanged)) { _ in
        controller.endHold()
      }
  }

  private func workspaceNotifications(_ name: Notification.Name) -> NotificationCenter.Publisher {
    NSWorkspace.shared.notificationCenter.publisher(for: name)
  }
}
