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
  private let capsLockLightController: CapsLockIndicatorController
  private let lidDisplaySleepController: LidDisplaySleepController

  internal var body: some Scene {
    WindowGroup("app.name") {
      SleepControlView(model: model)
        .onAppear(perform: applyRuntimeSettings)
        .onReceive(model.$isSleepDisabled, perform: applySleepState)
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
      menuBarLabel
    }
    .menuBarExtraStyle(.menu)

    Settings {
      SleepControlSettingsView(
        settings: shortcutSettings,
        isRegistered: hotKeyController.isRegistered,
        onShortcutChange: hotKeyController.register
      )
    }
  }

  private var terminationNotifications: NotificationCenter.Publisher {
    NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)
  }

  private var menuBarLabel: some View {
    MenuBarStatusIcon(model: model)
      .onAppear(perform: applyRuntimeSettings)
      .onReceive(model.$isSleepDisabled) { sleepDisabled in
        applySleepState(sleepDisabled)
      }
      .onReceive(shortcutSettings.$isCapsLockLightEnabled) { enabled in
        capsLockLightController.update(
          sleepDisabled: model.isSleepDisabled,
          isEnabled: enabled
        )
      }
      .onReceive(shortcutSettings.$isLidDisplaySleepEnabled) { enabled in
        lidDisplaySleepController.setEnabled(enabled)
      }
      .onReceive(terminationNotifications) { _ in
        capsLockLightController.restoreSystemCapsLockState()
        capsLockLightController.stop()
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
    capsLockLightController = CapsLockIndicatorController()
    lidDisplaySleepController = LidDisplaySleepController()
    initialModel.refresh()
    hotKeyController.register(initialSettings.shortcut)
    lidDisplaySleepController.setEnabled(initialSettings.isLidDisplaySleepEnabled)
    applySleepState(initialModel.isSleepDisabled)
  }

  private func applyRuntimeSettings() {
    hotKeyController.register(shortcutSettings.shortcut)
    lidDisplaySleepController.setEnabled(shortcutSettings.isLidDisplaySleepEnabled)
    applySleepState(model.isSleepDisabled)
  }

  private func applySleepState(_ sleepDisabled: Bool?) {
    ApplicationIconController.update(sleepDisabled: sleepDisabled)
    capsLockLightController.update(
      sleepDisabled: sleepDisabled,
      isEnabled: shortcutSettings.isCapsLockLightEnabled
    )
  }

  private func openSettings() {
    NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
    if #available(macOS 14.0, *) {
      NSApp.activate()
    }
  }

  private func quit() {
    capsLockLightController.restoreSystemCapsLockState()
    capsLockLightController.stop()
    NSApp.terminate(nil)
  }
}
