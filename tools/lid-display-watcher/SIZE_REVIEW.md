# Swift size and resident-memory review

Measured on Apple Silicon with Apple Swift 6.3.1.

| Metric | Result |
| --- | ---: |
| Release executable | 33,824 bytes |
| Direct-run idle RSS | 5,728-5,744 KiB |
| LaunchAgent idle RSS | 5,808-5,824 KiB |
| Idle CPU | 0.0% |
| Core line coverage | 100% |

## Build settings

The release build uses standard `-O`, whole-module optimization, and a single
compiler invocation for the core and executable sources. Linker settings merge
`__DATA_CONST` into `__DATA`, dead-strip unused code, omit the function-start
table, and export only `_main`. The final strip removes remaining symbols.

Executable responsibilities are split across entrypoint, IOKit notification,
display-command, and FFI-binding files for readability. Whole-module
optimization compiles them together, so this source split does not add calls,
runtime dependencies, file size, or resident memory.

Standard `-O` was retained after measurement: `-Osize` produced an executable
8 bytes larger, while `-Ounchecked` and disabling reflection each produced one
16 bytes larger. `-Ounchecked` would also weaken safety checks.

## Source representation

The implementation uses `StaticString` for readable command and IOKit literals.
Its null-terminated UTF-8 storage is passed directly to C, avoiding both the
numeric byte tuples from the first optimized version and `String.withCString`
runtime references. Command vectors, the empty child environment, and the
256-byte Mach receive buffer remain stack allocated.

The blocking `mach_msg` receive path is event-driven: it sleeps in the kernel
until IOKit sends a notification. CoreFoundation is used only to read and
release the initial lid-state registry value.

## Variable-storage audit

The release Mach-O contains exactly 1 byte in `__data`. This is the only
persistent mutable value, `previousLidState`; Swift reports `Optional<Bool>` as
size 1, stride 1, and alignment 1. It cannot represent the required unknown,
open, and closed states in less addressable storage.

Production source now has only three `var` declarations. The other two are
short-lived output locations that C APIs require: the child PID written by
`posix_spawn` and the registration token written by IOKit. Command argument
tuples and the empty environment are immutable `let` values. A `borrowing`
generic passes each tuple to the synchronous spawn call without copying it.

Static command and IOKit names are emitted directly into `__cstring`, without
heap allocation or per-variable runtime storage. Short-lived pointer, status,
and service variables are optimized into registers or stack slots used only
during setup or an event callback. Removing their source-level names would not
reduce idle RSS or the final artifact, and would make the FFI lifetime rules
harder to audit. The one-event receive helper limits their lifetime while also
keeping maximum brace nesting at three.

## Quality gates

`swift-format` and SwiftLint run with strict project configurations. SwiftLint's
`nesting` rule inspects closures and statements and caps function nesting at
three levels. Compiler lint enables complete concurrency checking and treats
warnings, concurrency diagnostics, implicit overrides, and soft-deprecation
diagnostics as errors. Thirteen dependency-free tests cover all 12 transition
input combinations plus a sequential event stream, and `llvm-cov` fails below
95% line coverage. The release build also fails above 34 KiB (34,816 bytes).

SwiftLint is installed as a development CLI and is absent from `Package.swift`.
Adding it did not change the 33,824-byte release artifact, its three dynamic
dependencies, or its 1-byte mutable `__data` section.
