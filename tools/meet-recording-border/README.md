# Recording Border for Google Meet

A Chrome extension that frames the Google Meet window with a colored border and a label showing
whether the meeting is recorded, transcribed, or summarized by Gemini. It is prepared for the
Chrome Web Store; see [Publish](#publish). Not affiliated with Google.

| State          | Border       | Label (en / ja)                                           |
| -------------- | ------------ | --------------------------------------------------------- |
| Before joining | gray, static | Before joining (checking status) / 参加前（状態を確認中） |
| No indicator   | blue         | Not recording / 録画なし                                  |
| Gemini notes   | purple       | Gemini taking notes / Gemini メモ作成中                   |
| Transcription  | orange       | Transcribing / 文字起こし中                               |
| Recording      | red          | ● Recording / ● 録画中                                    |

Several active features are joined in the label. For the color, recording outranks transcription,
which outranks Gemini notes.

## Install for development

```sh
bun install
bun run build   # writes dist/content.js (ignored by Git)
```

Open `chrome://extensions`, enable Developer mode, choose **Load unpacked** and select this
directory. After rebuilding, press the extension's reload button and reload the Meet tab. Tabs
left open keep the previous script until reloaded; it notices the reload, stops, and removes its
border instead of throwing "Extension context invalidated".

## How it works

- Meet has no recording API, so `src/detection/state/` matches Meet's on-screen labels
  (`aria-label` and `data-tooltip`) and visible text against `src/detection/phrases/<lang>.json`.
  Every language's phrases are combined because Meet's UI language can differ from the browser's.
  The transcription phrases are not yet verified against a real call.
- A visible line reading `call_end` (the leave-call button's icon ligature) marks the in-call
  state. For 10 seconds after joining,
  the pre-join notice ("Meet will record…") stands in for in-call indicators that render late.
- Each check reads the labels and visible text into one string and runs each pattern once.
- `src/scheduling/` checks only after the page changes, at most every 500 ms in idle time, and
  never while the tab is hidden. The observer is detached while a check is pending and in hidden
  tabs, so Meet's constant DOM updates create no mutation records meanwhile.
- `src/overlay/` draws in a closed shadow root. All numbers and colors are CSS custom properties
  in `:host`; only `opacity` animates.
- `src/content-script/` wires it together. Label text comes from `_locales/` through
  `chrome.i18n` (English by default, Japanese).

## Layout

| Path                                   | Contents                                                                              |
| -------------------------------------- | ------------------------------------------------------------------------------------- |
| `manifest.json`, `_locales/`, `icons/` | Extension manifest, localized name and labels, icons (`icon.svg` is the source)       |
| `src/`                                 | Content script source, grouped by job; tests sit next to each file                    |
| `scripts/`                             | esbuild build that minifies the JS and the inlined CSS into `dist/content.js`         |
| `stylelint-plugin/`                    | Local stylelint rules for the overlay CSS                                             |
| `store/`                               | Chrome Web Store listing text (`listing/`) and images (`images/<lang>/`, SVG sources) |
| `PRIVACY.md`                           | Privacy policy linked from the store listing                                          |

## Checks

```sh
bun run check   # oxfmt, oxlint, stylelint, tsc, vitest with 95% per-file coverage, build
```

`stylelint-plugin/` adds two rules for the overlay CSS: a rule declares either only custom
properties or only styles, and `:host` tokens hold literal values without `var()`.

## Publish

1. Raise `version` in `manifest.json` (keep `package.json` equal).
2. Run `bun run package`. It runs every check, then writes
   `release/recording-border-for-google-meet-<version>.zip` containing only `manifest.json`,
   `_locales/`, `dist/` and the icon PNGs.
3. Upload the ZIP in the Chrome Web Store Developer Dashboard and fill in the fields from
   [`store/listing/listing.md`](store/listing/listing.md), including the privacy practices and
   the pre-submission checklist.

After editing an SVG, regenerate the PNGs with `bun run icons` or `bun run store:images`
(requires `rsvg-convert`, for example `brew install librsvg`).
