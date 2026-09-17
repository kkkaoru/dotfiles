# Synthetic media fixture

`black.mp4` was generated locally with Apple's AVAssetWriter, not copied from a
user download or third-party video. It contains one opaque black 16×16 BGRA frame,
no audio, and a one-second presentation session. AVOutputSettingsAssistant's
640×480 preset was used with width/height changed to 16; its nominal frame rate
is 15 fps. The frame was filled with a cropped solid-black CIImage via CIContext.
No raw uninitialized pixel memory was encoded.

The test checks decoded dimensions, duration, nominal rate, absence of audio,
first-frame decoding, serialization and that the fixture bytes remain unchanged.
This intentionally does not prove full-stream validation or any Apple editor's
import/export behavior. ffmpeg is not a test or runtime dependency.

## Continuous 30-frame quadrants

`quadrants-30.mp4` is a separate, locally generated synthetic 16×16 fixture:
red/green upper quadrants and blue/yellow lower quadrants, 30 decoded frames,
30 fps, exactly one second, and no audio. It contains no user media.

The old runtime fixture performed two AVAssetExportSession exports before the
editor test. Under Address Sanitizer it produced only 29 source frames despite
reporting one second at 30 fps. The source prerequisite correctly failed; the
editor's 45-frame output assertion was not reached. A golden fixture removes
that independent, nondeterministic export from setup. The 30-source-frame and
45-edited-frame assertions remain unchanged, and every copied fixture is fully
decoded through the native reader before use. No assertion or sanitizer is waived.

One-time generation (not executed during tests):

```sh
ffmpeg -nostdin -n -f lavfi -i 'color=red:s=16x16:r=30:d=1,drawbox=x=8:y=0:w=8:h=8:color=lime:t=fill,drawbox=x=0:y=8:w=8:h=8:color=blue:t=fill,drawbox=x=8:y=8:w=8:h=8:color=yellow:t=fill' -an -c:v libx264 -profile:v high -crf 18 -bf 0 -pix_fmt yuv420p -frames:v 30 -video_track_timescale 30000 quadrants-30.mp4
```

Independent ffprobe frame counting verifies the stored source separately from
the native reader and editor under test. A lossless H.264 first attempt passed
FFmpeg decoding but Apple's reader rejected it; it was retained privately as
failure evidence, not adopted as the fixture. Native ASan verification of the
High-profile replacement and the full release gates remain required.
