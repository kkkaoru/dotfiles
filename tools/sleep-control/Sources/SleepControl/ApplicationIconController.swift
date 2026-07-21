import AppKit

/// Keeps the process icon synchronized with the resolved system setting.
@MainActor
internal enum ApplicationIconController {
  internal static func update(sleepDisabled: Bool?) {
    guard
      let iconURL = Bundle.main.url(
        forResource: resourceName(sleepDisabled: sleepDisabled),
        withExtension: "icns"
      ),
      let icon = NSImage(contentsOf: iconURL)
    else {
      return
    }

    NSApplication.shared.applicationIconImage = icon
  }

  private static func resourceName(sleepDisabled: Bool?) -> String {
    sleepDisabled == false ? "AppIconSleepEnabled" : "AppIcon"
  }
}
