import AppKit
import SleepControlUI

extension SleepControlSnapshots {
  internal static func testMenuTitles() throws {
    try withExtendedLifetime(MenuTitleWorkaround()) {
      try testTitleUpdates()
    }
    print("Menu title regression checks: passed")
  }

  private static func testTitleUpdates() throws {
    let menu = NSMenu(title: "Settings")
    let item = NSMenuItem(
      title: "設定…",
      action: #selector(NSApplication.terminate),
      keyEquivalent: ","
    )
    menu.addItem(item)
    guard
      item.title == "設定…",
      item.description.contains("設定…"),
      item.action == #selector(NSApplication.terminate),
      item.keyEquivalent == ","
    else {
      throw SnapshotError.menuTitleRegression
    }

    item.title = "スリープ制御について"
    guard item.description.contains("スリープ制御について") else {
      throw SnapshotError.menuTitleRegression
    }
    item.title = "設定 👩🏽‍💻 e\u{301}"
    guard item.description.contains("設定 👩🏽‍💻 e\u{301}") else {
      throw SnapshotError.menuTitleRegression
    }
    item.title = "Settings…"
    guard item.description.contains("Settings…") else {
      throw SnapshotError.menuTitleRegression
    }
    try testMenuTitleNotifications(menu: menu, item: item)
  }

  private static func testMenuTitleNotifications(menu: NSMenu, item: NSMenuItem) throws {
    let center = NotificationCenter.default
    center.post(name: NSMenu.didChangeItemNotification, object: nil)
    center.post(name: NSMenu.didChangeItemNotification, object: menu)
    center.post(
      name: NSMenu.didChangeItemNotification, object: menu, userInfo: ["NSMenuItemIndex": -1]
    )
    center.post(
      name: NSMenu.didChangeItemNotification, object: menu, userInfo: ["NSMenuItemIndex": 1]
    )
    guard item.title == "Settings…" else {
      throw SnapshotError.menuTitleRegression
    }
    item.title = ""
    let separator = NSMenuItem.separator()
    menu.addItem(separator)
    guard item.title.isEmpty, !item.description.isEmpty, separator.isSeparatorItem else {
      throw SnapshotError.menuTitleRegression
    }
  }
}
