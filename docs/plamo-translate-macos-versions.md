# PLaMo translate on other macOS versions

`./scripts/setup-plamo-translate.sh` takes its Darwin branch on every Mac: `mlx-lm` through `uv`,
the 4-bit model from `kkk4oru/plamo-3-translate-0.8b-mlx-4bit-fast`, the Python server as the
fallback, and then `tools/plamo-swift` as the default server when it builds. Which of those steps
can succeed depends on the macOS version and on the chip; nothing in the mods does.

Floors this document relies on, all read from the artifacts rather than assumed:

- `mlx` (Python): 0.32.2 and 0.32.4 publish `macosx_14_0_arm64`, `macosx_15_0_arm64` and
  `macosx_26_0_arm64` wheels, and every macOS wheel is arm64-only. `mlx-lm 0.32.2` requires
  `mlx>=0.32.2` and Python >= 3.11.
- `tools/plamo-swift`: `// swift-tools-version:6.0`, `platforms: [.macOS(.v15)]`, and `mlx-swift`
  0.33.0 whose own floor is macOS 14. `Sources/` has no `#available` gate, so the package floor is
  the only version check.

| macOS | MLX wheel | `plamo-swift` | What the mods run |
| --- | --- | --- | --- |
| 26.x, 27.x | `macosx_26_0_arm64` | builds | Swift server |
| 15.x | `macosx_15_0_arm64` | builds (the package floor) | Swift server |
| 14.x | `macosx_14_0_arm64` | cannot build | Python server |
| 13.x or older | none | cannot build | llama.cpp, by hand |
| Intel Mac | none | cannot build | llama.cpp, by hand |

The order the mods pick a server in is fixed in `commandSpec`: `~/.local/share/plamo-swift/plamo-swift`
first, then `~/.local/bin/plamo-mlx-server`, then
`~/.local/share/plamo-translate/llama.cpp/llama-server`. So the table's last column follows from
which binaries exist, not from an OS check.

## macOS 26 and later (Tahoe, 27)

Xcode 26 moved the Metal compiler out of the Xcode install into a downloadable component, so a
release build of `plamo-swift` fails until that component is present. Fetch it once per Xcode:

```sh
xcodebuild -downloadComponent MetalToolchain
./scripts/setup-plamo-translate.sh
```

`install_swift` reads the toolchain `xcrun --find metal` resolves and passes it as `TOOLCHAINS`.
When that is not the one you want, name it instead:

```sh
PLAMO_SWIFT_TOOLCHAINS=<toolchain bundle id> ./scripts/setup-plamo-translate.sh
```

Measured on this Mac (M5 Pro, macOS 27.2.1, Xcode 27.2, Metal from
`MetalToolchain-v27.2.47.0`): `mlx` installs as the `cp312-cp312-macosx_26_0_arm64` wheel, and the
Swift build is the one `tools/plamo-swift/README.md` measures — 0.063 s cold to ready, 2.50–5.29 ms
lukewarm to warm, 57 MB lukewarm footprint.

## macOS 15 (Sequoia)

The same commands, without the toolchain download: the Metal compiler ships inside Xcode 16, and
`mlx` has a `macosx_15_0_arm64` wheel. `plamo-swift` sits exactly on the package floor, so the
packed cache, the lukewarm state and the 2.50–5.29 ms reload all apply unchanged — none of them
depends on the OS version.

## macOS 14 (Sonoma)

`swift build` fails on the package's macOS 15 floor, `install_swift` prints its note and returns,
and the setup still finishes: the mods use `~/.local/bin/plamo-mlx-server`, i.e. the Python
`mlx_lm.server` on the same weights. `mlx` has a `macosx_14_0_arm64` wheel, so that server works.

What that costs, from the mod's own measurements: no packed cache and no lukewarm state, so every
start reads the weights again, and the first translation pays about 2.8 s where the Swift server is
0.15 s to ready and 0.17 s to its first token. Steady-state speed and output are equal.

## macOS 13 and older, and Intel Macs

No `mlx` wheel exists for these at the pinned version and MLX is Apple silicon only, so
`uv tool install mlx-lm==0.32.2` fails and the script stops at that step — nothing is left half
installed. `setup-plamo-translate.sh` installs llama.cpp only on Linux and Windows, so a Mac in
this group is outside what the script does:

1. Build llama.cpp with Metal, or take its prebuilt macOS release, into
   `~/.local/share/plamo-translate/llama.cpp/`.
2. Put `plamo-3-translate-0.8b-Q4_K_M.gguf` beside it, the same file the Linux and Windows path
   downloads.
3. The mods then find `llama-server` as the third entry of `commandSpec` and use it; no wrapper and
   no configuration is needed.

That path has no completion cache and no socket, so the mods reach it over `127.0.0.1:18555`.

## Checking a machine

```sh
command -v uv hf     # both required: uv from brew, hf via `uv tool install huggingface-hub`
ls ~/.local/bin/plamo-swift                     # present: the mods prefer the Swift server
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18555/v1/models
./scripts/plamo-swift-service.sh status         # macOS: launchd agent, port, footprint
```

The launchd agent is per machine and optional: `./scripts/plamo-swift-service.sh install` keeps one
warm server for both mods, and `reload` swaps in a new build in 0.49 s.
