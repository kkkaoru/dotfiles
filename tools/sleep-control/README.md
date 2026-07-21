# Sleep Control

A small native macOS GUI for the single system-wide setting controlled by:

```sh
sudo pmset -a disablesleep 0
sudo pmset -a disablesleep 1
```

The app reads `SleepDisabled` without privileges. A narrowly scoped sudoers
entry permits only the fixed `/usr/bin/pmset -a disablesleep 0` and `1`
commands, so changing the toggle does not request a password. It does not
change display sleep, start a background service, or modify
`lid-display-watcher`.

The UI and app name follow the macOS language setting. English and Japanese are
included. The Dock and app-switcher icon follows the current setting: a cyan
switch indicates enabled system sleep, while a gray switch indicates disabled
system sleep.

The UI uses positive switch semantics: ON enables Mac sleep and shows the moon;
OFF disables Mac sleep and shows the sun. Internally, the app still writes the
inverse `pmset` `disablesleep` value.

While running, the app remains available in the macOS menu bar. Its menu-bar
symbol changes between the moon and sun with the current setting. The global
sleep toggle defaults to `⌃⌥S`; open **Shortcut Settings…** from the menu to
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
17 dependency-free unit tests, a 95% core line-coverage gate, English and
Japanese UI snapshot rendering, strict concurrency, and ad-hoc code-signature
validation during bundle creation. Verification compares fresh renders with the
six checked-in UI state and settings images under `Snapshots/`. Run `make snapshots` to
intentionally update those baselines.

To remove it:

```sh
make uninstall
make uninstall-authorization
```
