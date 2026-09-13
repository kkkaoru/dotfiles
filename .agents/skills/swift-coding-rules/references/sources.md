# Swift rule research and rationale

Research date: **2026-09-13**. This file is supporting evidence, not a second normative rule set. The canonical rules are in [swift.md](swift.md).

## Version snapshot

Official search results identified Swift **6.3.3** as the latest tagged stable release, announced June 30, 2026. Apple also documents **Swift 6.4**, and the official release-process thread describes its release branches. Do not equate an announced or implemented feature with availability in a project's installed stable toolchain. This research does not establish a Swift 6.4 final-release date or require a toolchain upgrade.

Sources were checked through web search and readable/raw page retrieval. Some Apple/DocC pages require JavaScript and forum pages did not yield readable bodies; their indexed official passages were used where indicated below. Repository `main` documentation can describe newer behavior than a released toolchain. Recheck release tags, proposal implementation status, SDK availability, and local build settings before adopting version-sensitive APIs.

## Official sources

| Source | Findings used | Related rules |
| --- | --- | --- |
| [Swift 6.3.3 announcement](https://forums.swift.org/t/announcing-swift-6-3-3/87888) and [Swift releases](https://github.com/swiftlang/swift/releases) | Official indexed release information distinguishes stable tags from development snapshots. | SW01, SW10 |
| [Swift 6.4 release process](https://forums.swift.org/t/swift-6-4-release-process/85421) and [Apple: What's new in Swift](https://developer.apple.com/swift/whats-new/) | Official indexed material documents 6.4 work, including async defer, warning controls, noncopyable iteration, and Testing interoperability; availability remains toolchain-dependent. | SW04, SW16, SW30, SW32 |
| [SE-0493: Support async calls in defer bodies](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0493-defer-async.md) | Official indexed proposal is marked implemented in Swift 6.4; async cleanup is allowed in async enclosing contexts and awaited at scope exit. Do not repeat older blanket advice that defer can never await. | SW16 |
| [Swift 6.3 Released](https://www.swift.org/blog/swift-6.3-released/) | Retrieved release article: C interoperability, module selectors, optimization attributes, Swift Build preview, Testing warning issues and cancellation. Preview build tooling is not a reason to replace an existing build system; warnings/cancellation must not conceal failing tests. | SW01, SW04, SW18, SW30 |
| [Swift 6.2 Released](https://www.swift.org/blog/swift-6.2-released/) | Retrieved release article: optional default MainActor isolation, upcoming caller-context async behavior, explicit `@concurrent`, Span/InlineArray, and opt-in strict memory safety. These are not universal defaults for all Swift 6 projects. | SW18–SW21, SW25, SW30 |
| [Swift API Design Guidelines](https://www.swift.org/documentation/api-design-guidelines/) | Retrieved official guidance: clarity over brevity, call-site usage, naming, labels, and documentation. | SW08, SW09, SW13, SW29 |
| [The Swift Programming Language: Concurrency](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/concurrency/) ([retrieved source](https://github.com/swiftlang/swift-book/blob/main/TSPL.docc/LanguageGuide/Concurrency.md)) | Raw official documentation: structured task hierarchies, cooperative cancellation, unstructured/detached context inheritance, actor isolation, and possible interleaving at suspension. | SW19–SW26 |
| [Swift Testing README](https://github.com/swiftlang/swift-testing/blob/main/README.md) | Retrieved official README: built into Swift 6/Xcode 16 toolchains without a package dependency, expressive assertions, parameterized tests, default parallelism, and coexistence with XCTest. | SW32–SW34 |
| [swift-format README](https://github.com/swiftlang/swift-format/blob/main/README.md) | Retrieved official README: `swift format` ships in Swift 6/Xcode 16+; configuration is project-specific and its defaults are not a universal official style guide. | SW03 |
| [SwiftPM swift test reference](https://docs.swift.org/latest/documentation/packagemanagerdocs/swifttest/) | Official indexed CLI reference documents code coverage, filters, sanitizer options, and coverage-path reporting. Coverage percentages are local policy, not SwiftPM requirements. | SW03, SW35, SW36 |
| [SwiftUI task lifecycle](https://developer.apple.com/documentation/swiftui/view/task(name:priority:file:line:_:)) and [Task](https://developer.apple.com/documentation/swift/task) | Official indexed API passages describe view-lifetime cancellation, task handles, and cooperative cancellation. Exact signatures vary with SDK version. | SW17, SW22, SW23, SW27 |

## Adaptation from the existing language skills

- Follow `python-coding-rules` and `rust-coding-rules`: a small `SKILL.md`, one English normative reference, a Japanese human translation, and unique rule IDs without duplicated requirements.
- Retain the three existing skills' emphasis on implementation planning, focused functions, minimal nesting, explicit contracts, removing dead code, English developer diagnostics, verification after logical changes, test ownership, and final rules review.
- Following the user's explicit coverage adjustment, require at least 95% per changed testable file for lines/functions and for branches where supported. Lower existing thresholds do not waive it; unmeasurable code or missing tooling requires an explicitly approved exception. This is user-directed repository policy, not an Apple recommendation.
- Do not mechanically import TypeScript-specific restrictions: Swift needs `var` for legitimate mutation, enums for domain states, inference for readable code, and argument labels rather than mandatory object parameters.
- Preserve explicit test expectations and discourage hidden helper logic, but permit Swift Testing's native parameterized cases instead of banning the framework's intended data-driven model. Inputs and expected values remain explicit case data.
- Respect existing frameworks and supported platforms: Swift Testing is a preference for suitable new unit-test projects, not a mandatory rewrite of XCTest suites or a replacement for UI/performance tooling.
- Add Swift-specific safeguards for optionals, recoverable errors, ARC ownership, unsafe memory boundaries, configurable isolation, sendability, structured task lifetimes, cancellation, actor reentrancy, continuation completion, and SwiftUI state ownership.
- Apply strict completion gates: zero warnings/errors in affected targets, actual verification evidence, relevant sanitizer runs, and no unverified completion. Unsafe memory operations, force unwraps/casts, unchecked concurrency, detached tasks, unowned references, and other prohibited constructs cannot be justified by convenience. Exceptions require the user's prior explicit approval, documented necessity, minimal scope, compensating tests, and a removal condition. This policy is intentionally stricter than the language itself and does not authorize changing existing project settings or unrelated code.

## Maintenance

When refreshing this skill, consult the official release pages and accepted/implemented Swift Evolution proposals, then update the English rules and Japanese translation together while preserving matching IDs. Keep new compiler syntax, SDK APIs, and runtime deployment support distinct. Do not encode a moving “latest” toolchain as a mandatory project baseline.
