// Runs with bun: bundled into dist/content.js by `bun run build` as the extension's content script.
import { compilePatterns } from "../detection/state/meet-state.ts";
import { start } from "../overlay/overlay.ts";
// Shortcut: Meet phrases exist for en/ja only and the transcription ones are guessed.
// Add src/detection/phrases/<lang>.json for other UI languages and verify against a real call.
import englishPhrases from "../detection/phrases/en.json" with { type: "json" };
import japanesePhrases from "../detection/phrases/ja.json" with { type: "json" };

start({
  page: document,
  // Chrome picks the locale from the browser UI language, falling back to manifest default_locale (en).
  translate: (key) => chrome.i18n.getMessage(key),
  patterns: compilePatterns([englishPhrases, japanesePhrases]),
  // Chrome clears chrome.runtime.id when the extension context is invalidated.
  isAlive: () => chrome.runtime?.id !== undefined,
});
