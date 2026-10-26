# Recording Border for Google Meet

A Chrome extension that frames the Google Meet window with a colored border and a label showing
whether the meeting is recorded, transcribed, or summarized by Gemini. It is prepared for the
Chrome Web Store but not yet published; see [Publish](#publish). Not affiliated with Google.

| State          | Border       | Label (en / ja)                                           |
| -------------- | ------------ | --------------------------------------------------------- |
| Before joining | gray, static | Before joining (checking status) / 参加前（状態を確認中） |
| No indicator   | blue         | Not recording / 録画なし                                  |
| Gemini notes   | purple       | Gemini taking notes / Gemini メモ作成中                   |
| Transcription  | orange       | Transcribing / 文字起こし中                               |
| Recording      | red          | ● Recording / ● 録画中                                    |

Several active features are joined in the label, for example `● Recording + Gemini taking notes`.
For the color, recording outranks transcription, which outranks Gemini notes. The border shows
only on meeting pages (`xxx-xxxx-xxx`), not on the Meet home page.

## Permissions and privacy

The manifest requests no `permissions` and no `host_permissions`. Its only access is one content
script on `https://meet.google.com/*` (top frame, `document_idle`). It reads what the page already
shows, makes no network requests, and stores nothing; see [`PRIVACY.md`](PRIVACY.md).

## Requirements

- Chrome 120 or newer (`minimum_chrome_version`).
- [Bun](https://bun.sh) for scripts; Node.js runs stylelint and vitest.
- `rsvg-convert` only to regenerate PNGs from SVG (`brew install librsvg`).

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

- **Detection** (`src/detection/state/`): Meet has no recording API, so each check reads the
  page's `aria-label` and `data-tooltip` values plus its visible text (`innerText`) into one
  string and tests one pattern per feature against it. The patterns come from
  `src/detection/phrases/<lang>.json`; every language is combined because Meet's UI language can
  differ from the browser's. Patterns are case-insensitive and multiline, so `^rec$` matches one
  line.
- **In call**: a visible line reading `call_end` (the leave-call button's icon ligature) marks the
  in-call state in every UI language. For 10 seconds after joining, the pre-join notice ("Meet will
  record…") stands in for in-call indicators that render late.
- **Scheduling** (`src/scheduling/`): a `MutationObserver` triggers a check only after the page
  changes, at most every 500 ms and in idle time (`requestIdleCallback`). The observer is detached
  while a check is pending and in hidden tabs, so Meet's constant DOM updates create no mutation
  records meanwhile; an unchanged page costs nothing.
- **Overlay** (`src/overlay/`): drawn in a closed shadow root, so Meet's CSS and ours cannot touch
  each other. Every number and color is a CSS custom property in `:host`; `data-tone` swaps the
  color, the DOM is written only when the state changes, and only `opacity` animates on its own
  compositor layer. The pulse stops before joining and when the system reduces motion.
- **Entry** (`src/content-script/`): wires the phrases, `chrome.i18n` labels from `_locales/`
  (English by default, Japanese) and the extension-alive check together.

## Limitations

- Phrases exist for English and Japanese Meet only. The transcription phrases are guessed and not
  yet verified against a real call; in-call detection and the recording badge (`REC`) were checked
  with simulated elements, not a real recorded call.
- Detection depends on Meet's on-screen text and icon names and breaks if Meet changes them. The
  English "Recording" tool title is deliberately not a phrase, so opening the tools panel does not
  turn the border red.

### Adding a Meet UI language

1. Add `src/detection/phrases/<lang>.json` with regular-expression sources for `recording`,
   `transcription` and `gemini`, and import it in `src/content-script/content.ts`.
2. For a new label language, add `_locales/<lang>/messages.json` with the same keys as `en`.

## Layout

| Path                                   | Contents                                                                              |
| -------------------------------------- | ------------------------------------------------------------------------------------- |
| `manifest.json`, `_locales/`, `icons/` | Extension manifest, localized name and labels, icons (`icon.svg` is the source)       |
| `src/`                                 | Content script source, grouped by job; tests sit next to each file                    |
| `scripts/`                             | esbuild build that minifies the JS and the inlined CSS into `dist/content.js`         |
| `@types/`                              | Declares `*.css` imports as text                                                      |
| `stylelint-plugin/`                    | Local stylelint rules for the overlay CSS                                             |
| `store/`                               | Chrome Web Store listing text (`listing/`) and images (`images/<lang>/`, SVG sources) |
| `PRIVACY.md`                           | Privacy policy linked from the store listing                                          |

Source directories hold either only files or only subdirectories, following the repository's
TypeScript coding rules (`.agents/skills/typescript-coding-rules/`).

## Scripts

| Command                | What it does                                                                             |
| ---------------------- | ---------------------------------------------------------------------------------------- |
| `bun run build`        | Bundles `dist/content.js` with esbuild, minifying the JS and the inlined CSS             |
| `bun run check`        | `format:check`, `lint`, `tsc`, `test` and `build`; run before every commit               |
| `bun run test`         | vitest with happy-dom; fails below 95% lines, branches, functions or statements per file |
| `bun run lint`         | oxlint (type-aware) and stylelint                                                        |
| `bun run format`       | oxfmt                                                                                    |
| `bun run package`      | `check`, then the store ZIP in `release/`                                                |
| `bun run icons`        | Renders `icons/icon.svg` to 16, 32, 48 and 128 px PNGs                                   |
| `bun run store:images` | Renders `store/images/*/*.svg` to PNGs                                                   |

The build minifies CSS with esbuild because Bun's minifier rounds large numbers
(`2147483647` became `2147480000`, lowering the overlay's `z-index`).

stylelint extends `stylelint-config-standard` and `stylelint-config-recess-order`, forbids
low-performance animations and hard-coded values, and limits selectors, units and at-rules.
`stylelint-plugin/` adds two rules: a rule declares either only custom properties or only styles,
and `:host` tokens hold literal values without `var()`. `!important` is allowed.

Tests replace what happy-dom lacks or runs on its own timers: `requestIdleCallback` and
`MutationObserver` are stubbed, `hidden` text uses `display: none`, and the CSS custom properties
inherited from `:host` are checked by parsing the stylesheet.

## Publish

1. Raise `version` in `manifest.json` (keep `package.json` equal); the ZIP is named after the
   manifest version.
2. Run `bun run package`. It runs every check, then writes
   `release/recording-border-for-google-meet-<version>.zip` containing only `manifest.json`,
   `_locales/`, `dist/` and the icon PNGs.
3. Upload the ZIP in the Chrome Web Store Developer Dashboard and fill in the fields from
   [`store/listing/listing.md`](store/listing/listing.md), including the privacy practices and
   the pre-submission checklist.
