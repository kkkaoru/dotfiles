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
