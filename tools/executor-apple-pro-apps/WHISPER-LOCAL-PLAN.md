# Local Whisper recognition contract

The installed SpeechAnalyzer path now returns native timed lexical groups and
passes its existing full gates, but bounded real Japanese checks still contain
recognition errors. Clean vocals and explicit contextual vocabulary did not fix
those errors. Do not label that experiment an accuracy improvement.

Evaluate WhisperKit via the same Executor Desktop native integration, not a
second server or a cloud transcription service. The pinned dependency is
`argmaxinc/argmax-oss-swift` 1.1.0 (WhisperKit product only). Initial model candidate:
`argmaxinc/whisperkit-coreml`, revision
`0f63a7800b00dd0226abd051b906c246e1907482`,
`openai_whisper-large-v3-v20240930_turbo` (24 files / 1,638,464,446 bytes).
Store models and provenance privately, not in Git/package artifacts.

## Isolation and I/O

- New bounded read-only measurement tool; explicit local audio, model and tokenizer
  directories. Japanese transcription first; no auto language detection, microphone,
  app launch, model installation or authentication in inference.
- A disposable native child owns each model instance and its memory. Requests and
  returned DTOs are Sendable; no global mutable model or unchecked Sendable wrapper.
  Loading/inference receive cancellation checks and an outer hard process deadline.
- `WhisperKitConfig(download: false, load: false, prewarm: false)` is mandatory.
  Inject a strictly local tokenizer before loading models.
- **Do not use `ModelUtilities.loadTokenizer`**: its local-error path silently
  downloads from the Hub. The upstream `WhisperTokenizerWrapper` initializer is
  internal. Implement the public `WhisperTokenizer` protocol with the public
  `AutoTokenizerWrapper.from(modelFolder:..., strict: true)` local-only loader.
  Audit that loader's local configuration path; never call its pretrained overload.
  Override the model's tokenizer-loading method to fail if injection is absent.
- Require all special tokens from the supplied vocabulary, with no guessed numeric
  token IDs. Preserve Unicode byte-token groups for alignment. Missing/corrupt
  local assets must fail, not trigger remote fallback.
- Request word timestamps; reject missing lexical timing and keep raw output.
  Native/Whisper timing remains an ASR estimate, not human-reviewed forced alignment.

## Verification and integration

1. Verify pinned model bytes by size plus upstream LFS SHA-256 or Git blob hash.
   Record the independent OpenAI tokenizer revision and verify its local JSON files.
2. Test local-only missing/corrupt paths, Unicode grouping, special-token validation,
   timing budgets, cancellation and outer deadline dispatch; use installed approved
   assets for native synthetic Japanese tests, not private recordings as fixtures.
3. Repeat strict format/warnings, >=95% line/function coverage per production file,
   full separate TSan/ASan, normal restored coverage and release build after changes.
4. Discover the new tool through Executor and compare bounded source/noisy and
   separated-voice samples. Do not infer a word-error rate without ground truth.
5. Select the backend on actual evidence, then finish the reusable caption pipeline
   and new full render. Source preservation and complete decode remain separate gates.
