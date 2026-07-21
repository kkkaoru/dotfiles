import AppKit
import SleepControlCore
import SleepControlUI
import SwiftUI

@main
@MainActor
internal struct SleepControlApp: App {
  @StateObject private var model: SleepSettingsModel
  @StateObject private var shortcutSettings: ShortcutSettingsStore
  @StateObject private var hotKeyController: GlobalHotKeyController

  internal var body: some Scene {
    WindowGroup("app.name") {
      SleepControlView(model: model)
        .onAppear {
          hotKeyController.register(shortcutSettings.shortcut)
        }
    }
    .windowResizability(.contentSize)

    MenuBarExtra {
      MenuBarContentView(
        model: model,
        shortcut: shortcutSettings.shortcut,
        openSettings: openSettings,
        quit: quit
      )
    } label: {
      MenuBarStatusIcon(model: model)
        .onReceive(model.$isSleepDisabled) { sleepDisabled in
          ApplicationIconController.update(sleepDisabled: sleepDisabled)
        }
    }
    .menuBarExtraStyle(.menu)

    Settings {
      ShortcutSettingsView(
        settings: shortcutSettings,
        isRegistered: hotKeyController.isRegistered,
        onShortcutChange: hotKeyController.register
      )
    }
  }

  internal init() {
    let initialModel = SleepSettingsModel(client: SystemSleepSettingsClient())
    let initialSettings = ShortcutSettingsStore()
    let initialHotKey = GlobalHotKeyController {
      initialModel.toggleSleep()
    }
    _model = StateObject(wrappedValue: initialModel)
    _shortcutSettings = StateObject(wrappedValue: initialSettings)
    _hotKeyController = StateObject(wrappedValue: initialHotKey)
    initialModel.refresh()
  }

  private func openSettings() {
    // SwiftUI's Settings scene exposes this responder-chain action on macOS 13.
    NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
    if #available(macOS 14.0, *) {
      NSApp.activate()
    }
  }

  private func quit() {
    NSApp.terminate(nil)
  }
}
