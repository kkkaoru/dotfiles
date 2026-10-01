import AppKit

/// Keeps menu titles in Cocoa storage to avoid macOS 26's Swift UTF-16 description crash.
@MainActor
public final class MenuTitleWorkaround: NSObject {
  private var isUpdatingTitle = false

  /// Starts before SwiftUI creates menus, including the standard Settings command and pickers.
  override public init() {
    super.init()
    for name in [NSMenu.didAddItemNotification, NSMenu.didChangeItemNotification] {
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(repairTitle),
        name: name,
        object: nil
      )
    }
  }

  @objc
  private func repairTitle(_ notification: Notification) {
    guard
      !isUpdatingTitle,
      let menu = notification.object as? NSMenu,
      let index = notification.userInfo?["NSMenuItemIndex"] as? Int,
      menu.items.indices.contains(index)
    else {
      return
    }
    let item = menu.items[index]
    let title = NSMutableString(string: item.title)
    isUpdatingTitle = true
    defer { isUpdatingTitle = false }
    // NSMenuItem ignores equal titles: clear first so the native copy actually replaces storage.
    item.title = ""
    item.title = title as String
  }

  // The observer is weakly held by NotificationCenter and lives for the app's menu lifetime.
  deinit {
    NotificationCenter.default.removeObserver(self)
  }
}
