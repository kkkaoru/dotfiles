# C# and Unity rule research and rationale

Research date: **2026-10-18**. This file is supporting evidence, not a second normative rule set. The canonical rules are in [csharp.md](csharp.md).

## Version snapshot

- Mono-based Unity 6 releases compile with Roslyn at **C# 9.0**. Unsupported C# 9 features include init-only setters, covariant return types, module initializers, extensible calling conventions, and suppressing `localsinit`. Records need a user-declared `IsExternalInit` and must not be used in serialized types.
- The default Unity 6 API Compatibility Level is **.NET Standard 2.1**; the alternative is .NET Framework 4.8 plus Unity additions. .NET 5+ BCL APIs are therefore unavailable.
- Unity's 2026 status updates describe an experimental CoreCLR player in 6.7, and a CoreCLR editor with .NET 10 and C# 14 targeted for 6.8. Fast Enter Play Mode (no domain reload) became the default in 6.6. These are roadmap targets: recheck the project's actual version before relying on newer language or BCL features.
- `MonoBehaviour.destroyCancellationToken` was added in 2022.2; `Awaitable` arrived in the 2023.1 cycle (2023.1 is supported only by secondary sources).
- The Unity Code Coverage package does not implement branch coverage or NPath complexity; both always report zero. Line coverage is reported, in OpenCover format.

## Sources

| Source | Findings used | Related rules |
| --- | --- | --- |
| [Unity Manual: C# compiler](https://docs.unity3d.com/Manual/csharp-compiler.html) ([6000.0](https://docs.unity.com/en-us/engine/6000.0/manual/programming-environment/overview-of-dot-net-in-unity/csharp-compiler), [6000.3](https://docs.unity.com/en-us/engine/6000.3/manual/programming-environment/overview-of-dot-net-in-unity/csharp-compiler)) | C# 9.0, unsupported features, record caveats, no records in serialized types. | CS35, CS37 |
| [Unity Manual: .NET profile support](https://docs.unity3d.com/6000.1/Documentation/Manual/dotnet-profile-support.html) and [ApiCompatibilityLevel](https://docs.unity3d.com/6000.6/Documentation/ScriptReference/ApiCompatibilityLevel.html) | .NET Standard 2.1 default; .NET Framework 4.8 alternative. | CS01, CS35 |
| [CoreCLR status update, March 2026](https://discussions.unity.com/t/coreclr-scripting-and-ecs-status-update-march-2026/1711852) and [June 2026](https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299) | CoreCLR, .NET 10, and C# 14 targets; Fast Enter Play Mode default. | CS35, CS42 |
| [MonoBehaviour.destroyCancellationToken](https://docs.unity3d.com/kr/2023.2/ScriptReference/MonoBehaviour-destroyCancellationToken.html) and [Awaitable overview (secondary)](https://hackernoon.com/unity-20231-introduces-awaitable-class) | Lifetime-bound cancellation; `OperationCanceledException` on destroy. | CS40 |
| [Code Coverage technical details](https://docs.unity3d.com/Packages/com.unity.testtools.codecoverage@1.2/manual/TechnicalDetails.html) | Branch coverage is not implemented; EditMode/PlayMode in the editor only. | CS51 |

General .NET guidance (nullable reference types, naming, exceptions, `IDisposable`, async/cancellation, `IHttpClientFactory`) follows Microsoft Learn conventions known before this research; it was not re-fetched on this date.

## Adaptation from the existing language skills

- Follow the Swift skill's structure: a small `SKILL.md`, one English normative reference, a Japanese translation with matching IDs, and this research file.
- Keep the shared themes of the other language skills: planning, verification after each logical change, zero warnings, no suppression without prior approval, named constants, focused functions, minimal nesting, thin entry points, typed parameter groups, removing unused code, English diagnostics, non-DRY tests with literal expectations, test ownership, and a final rules review.
- Coverage uses the Swift skill's strict 95% per changed file. Unity's tooling cannot measure branches, so CS51 substitutes 95% line coverage, full coverage of changed public methods, and a test for every branch reviewed against the diff.
- Unity rules refine general rules only where the engine differs (toolchain, language level, `UnityEngine.Object` null semantics, serialization, threading, async, logging). Each refinement is referenced from the general rule rather than duplicated.

## Maintenance

When refreshing this skill, recheck the Unity C# compiler page, API profile page, CoreCLR status, and Code Coverage documentation for the versions in use. Update the English rules and the Japanese translation together, keeping matching IDs.
