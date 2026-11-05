# Sleep Control

A small native macOS GUI for the system-wide sleep setting controlled by:

```sh
sudo pmset -a disablesleep 0
sudo pmset -a disablesleep 1
```

The app reads `SleepDisabled` without privileges. A narrowly scoped sudoers
entry permits only the fixed `/usr/bin/pmset -a disablesleep 0` and `1`
commands, so changing the toggle does not request a password. The app also
optionally turns displays off when a MacBook lid closes, and can use the
built-in Caps Lock light as an indicator while system sleep is disabled.

The UI and app name follow the macOS language setting. English and Japanese are
included. The Dock and app-switcher icon follows the current setting: a cyan
switch indicates enabled system sleep, while a gray switch indicates disabled
system sleep.

The UI uses positive switch semantics: ON enables Mac sleep and shows the moon;
OFF disables Mac sleep and shows the sun. Internally, the app still writes the
inverse `pmset` `disablesleep` value. Settings contains independent switches
for lid-close display sleep, the hot-corner display hold and the Caps Lock
indicator, plus the global shortcut picker.

### Keeping displays off after a hot corner

Settings has a switch, on by default, that holds displays off after a hot corner
assigned to **Put Display to Sleep** turns them off. If they wake without keyboard
or mouse input, for example through another app's user-activity assertion, the app
runs `pmset displaysleepnow` again. It is event-driven, with no polling and no extra
permission: a display sleep starts a hold when the cursor is in such a corner, and a
wake is compared against the age of the last HID event and the HID idle time.

Input in the first 2 seconds of a hold counts as the hand still settling, so it turns
displays off again; later input ends the hold, and so does Touch ID at any time, which
the app recognizes by the biometric event in WindowServer's user-activity assertion name. A sensitive pointer that reports small
movements counts as input. Opening or closing the lid and system sleep also end the
hold, because the key press that wakes a sleeping Mac is not always delivered as an
event. System sleep itself is left to macOS: with displays off, normal idle sleep
continues unless something else prevents it.

Unlocking the screen, including with Touch ID, always ends the hold and wakes displays,
whether or not the switch is on. macOS does not say which method unlocked it.

### Low-battery sleep while the lid is closed

Settings has an independent **Low Battery Sleep** switch and a **0–90% slider in
10% steps**. Both are saved across launches; first use defaults to **enabled at
50%**. Zero means a real 0% cutoff, not disabled (use the switch to disable).
The app checks on native battery and lid notifications, launch, wake and settings
changes, including on AC power. There is no polling timer; notification bursts
coalesce into at most one pending check, processed serially. With the lid closed and charge at or below the cutoff, it
first enables system sleep using the existing authorization, rechecks the current
sensors and preferences, then requests `pmset sleepnow`. System sleep remains
enabled after wake; the previous sleep-disabled setting is not restored.

Missing/invalid sensors never trigger sleep. Errors are shown in Settings and
retried on the next event. No new sudoers permissions or Input Monitoring access
are needed for this feature. Automated tests use fake sleep commands; the system
smoke check only reads sensors and never sleeps the host.

macOS requires **Input Monitoring** permission for the Caps Lock hardware LED.
If the permission prompt was previously denied, add **Sleep Control** in
System Settings → Privacy & Security → Input Monitoring, then restart the app.

While running, the app remains available in the macOS menu bar. Its menu-bar
symbol changes between the moon and sun with the current setting. Clicking it
opens a compact popover rather than an AppKit-tracked menu, avoiding the stale
menu-item crash that macOS 26 can trigger after sleep or a display cycle.
Standard app menus and settings pickers also normalize their titles to Cocoa
string storage: on macOS 26.5.1, describing a Swift-backed Japanese `NSMenuItem`
can trap in `String.UTF16View` while highlighting or opening Settings. Menu title
regression checks exercise this exact description path, including title updates.
The global sleep toggle defaults to `⌃⌥S`; open **Settings…** from the popover to
choose another modifier combination and letter key. The Carbon hot-key API is
built into macOS and does not require Accessibility permission or polling.

## Build and install

Swift 6.2 or newer and SwiftLint 0.65 or newer are required.

```sh
make verify
make install-authorization
make install
```

`make install-authorization` requests administrator authentication once,
validates the generated rule with `visudo`, and installs it as
`/etc/sudoers.d/sleep-control`. Any process running as the current user can then
invoke those two exact `pmset` commands without a password; no other root
command or `pmset` argument is permitted by this rule.

`make install` copies `Sleep Control.app` to `~/Applications` and opens it.
The app itself has no third-party or runtime package dependencies.
Verification includes strict formatting, all applicable SwiftLint opt-in rules,
63 dependency-free unit tests, menu-title regression checks, read-only system
power checks, a 95% core line-coverage gate, and 95% per-file line/function gates
for battery sleep and the menu workaround, plus English and
Japanese UI snapshot rendering, strict concurrency, and ad-hoc code-signature
validation during bundle creation. Verification compares fresh renders with the
twelve checked-in window, popover, and settings images under `Snapshots/`. Run `make snapshots` to
intentionally update those baselines.

To remove it:

```sh
make uninstall
make uninstall-authorization
```
