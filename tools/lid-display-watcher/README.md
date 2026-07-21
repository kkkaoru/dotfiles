# Lid Display Watcher

Turns all displays off when a MacBook lid closes and wakes them when it opens,
while allowing the Mac itself to remain awake. It receives the public IOKit
clamshell notification through a Mach port and performs no polling.

The production implementation is dependency-free Swift with direct FFI to
CoreFoundation, IOKit, and libSystem. It does not use Foundation, Process,
SwiftCore at runtime, third-party packages, timers, or a heap-backed event loop.

## Development

Apple Swift 6.2 or newer and SwiftLint 0.65 or newer are required.

```sh
brew install swiftlint
```

```sh
make format
make lint
make test
make coverage
make release
```

`make verify` runs lint, 13 tests, the 95% coverage gate, and the optimized
release build. The release fails if the executable exceeds 34 KiB. Linting
combines strict `swift-format`, strict SwiftLint, and Swift compiler
diagnostics-as-errors. It enables complete concurrency checking, all applicable
opt-in lint rules, prohibits force unwrap/force try/IUO, and limits nesting to
three levels.

SwiftLint is a development-only Homebrew CLI. It is not declared in
`Package.swift`, linked into the executable, or invoked by `make release`.

## Source layout

- `main.swift` contains only the process entrypoint.
- `LidNotificationService.swift` owns IOKit registration and the blocking Mach loop.
- `DisplayPowerController.swift` owns the `pmset` and `caffeinate` commands.
- `SystemBindings.swift` contains the audited macOS FFI boundary.
- `LidStateTransition.swift` contains the platform-independent tested logic.

## Build and run

```sh
make release
.build/release/lid-display-watcher
```

Running `pmset` alone does not start the watcher. Install its user LaunchAgent
once so it starts at login and is restarted if it exits:

```sh
make install
make status
```

`make install` places the executable in `~/.local/bin` and bootstraps
`com.kkkaoru.lid-display-watcher` in the current GUI login domain. Use
`make restart` after rebuilding or `make uninstall` to remove it.

System sleep can then be disabled independently:

```sh
sudo pmset -a disablesleep 1
```

`pmset displaysleepnow` affects external displays as well as the built-in
display. See [SIZE_REVIEW.md](SIZE_REVIEW.md) for the build and memory review.
