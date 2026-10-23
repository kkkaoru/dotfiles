# Chrome Web Store listing

Values to paste into the Chrome Web Store Developer Dashboard. The name and summary come from
`_locales/*/messages.json` inside the uploaded ZIP; keep them in sync with this file.

## Package

| Field          | Value                                                                                    |
| -------------- | ---------------------------------------------------------------------------------------- |
| Upload file    | `release/recording-border-for-google-meet-<version>.zip` from `bun run package`          |
| Version        | `version` in `manifest.json` (keep equal to `package.json`; raise both for every upload) |
| Minimum Chrome | 120                                                                                      |

## Store listing tab

| Field                 | English                                                                                                 | 日本語                                                                                    |
| --------------------- | ------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Name                  | Recording Border for Google Meet                                                                        | Recording Border for Google Meet                                                          |
| Summary (≤ 132 chars) | Shows Google Meet recording, transcription and Gemini note-taking status as a colored border and label. | Google Meet の会議画面に、録画・文字起こし・Gemini メモの状態を縁の色と文字で表示します。 |
| Category              | Communication                                                                                           | Communication                                                                             |
| Language              | English (default), Japanese                                                                             |                                                                                           |

### Description (English)

```text
Know at a glance whether your Google Meet call is being recorded.

Recording Border for Google Meet frames the meeting window with a colored border and a short label:

• Red — the meeting is being recorded
• Orange — the meeting is being transcribed
• Purple — Gemini is taking notes
• Blue — none of the above
• Gray — before you join (the pre-join notice is checked)

When several are active, the label lists all of them, for example "● Recording + Gemini taking notes".

Private by design
• Runs only on meet.google.com
• Reads only what Meet already shows on screen
• No network requests, no analytics, no data stored

Light on your computer
• Checks only when the page changes, in idle time, never in background tabs
• Only opacity animates; the pulse stops if your system reduces motion

The border relies on Meet's own on-screen indicators and is a visual aid; always confirm with Meet. Supports Meet in English and Japanese.

Not affiliated with or endorsed by Google. Google Meet is a trademark of Google LLC.
```

### Description (日本語)

```text
Google Meet の会議が録画されているかどうかが、ひと目でわかります。

Recording Border for Google Meet は、会議画面のまわりに色付きの縁と短いラベルを表示します。

• 赤 — 録画中
• オレンジ — 文字起こし中
• 紫 — Gemini がメモを作成中
• 青 — どれも動いていない
• グレー — 参加前（参加前の画面のお知らせを確認します）

複数が同時に動いているときは「● 録画中 + Gemini メモ作成中」のように、すべてを並べて表示します。

プライバシーに配慮
• meet.google.com でのみ動作
• Meet の画面にすでに表示されている内容だけを読み取ります
• 通信なし、アクセス解析なし、データの保存なし

パソコンにやさしい
• ページが変わったときだけ、空き時間に確認します。バックグラウンドのタブでは動きません
• アニメーションは透明度だけ。OS で「視差効果を減らす」を選ぶと点滅も止まります

縁は Meet 自身の表示をもとにした目安です。必ず Meet の表示も確認してください。英語と日本語の Meet に対応しています。

Google とは提携しておらず、Google の承認を受けたものではありません。Google Meet は Google LLC の商標です。
```

### Graphics

| Asset            | Size       | English                                       | 日本語                                        |
| ---------------- | ---------- | --------------------------------------------- | --------------------------------------------- |
| Store icon       | 128 × 128  | `icons/icon-128.png`                          | same                                          |
| Screenshots      | 1280 × 800 | `store/images/en/screenshot-1.png` … `-4.png` | `store/images/ja/screenshot-1.png` … `-4.png` |
| Small promo tile | 440 × 280  | `store/images/en/promo-small.png`             | `store/images/ja/promo-small.png`             |

Edit the SVG next to each PNG, then run `bun run store:images` (or `bun run icons`).

## Privacy practices tab

| Field                                                       | Value                                                                                                                                                                                                                   |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Single purpose                                              | Shows on the Google Meet screen whether the meeting is being recorded, transcribed, or summarized by Gemini.                                                                                                            |
| Host permission justification (`https://meet.google.com/*`) | The content script must read Google Meet's on-screen recording, transcription and Gemini indicators and draw the border on that page. It runs on no other site.                                                         |
| Other permissions                                           | None requested.                                                                                                                                                                                                         |
| Remote code                                                 | No. All code ships in `dist/content.js`; nothing is fetched or evaluated at runtime.                                                                                                                                    |
| Data usage                                                  | Collects none of the listed data types.                                                                                                                                                                                 |
| Certifications                                              | Check all three: no sale or transfer to third parties outside the approved use cases; no use or transfer for purposes unrelated to the single purpose; no use or transfer to determine creditworthiness or for lending. |
| Privacy policy URL                                          | https://github.com/kkkaoru/dotfiles/blob/master/tools/meet-recording-border/PRIVACY.md                                                                                                                                  |

## Distribution tab

| Field      | Value       |
| ---------- | ----------- |
| Visibility | Public      |
| Regions    | All regions |
| Price      | Free        |

## Before submitting

- [ ] Developer account registered (one-time fee) and contact email verified.
- [ ] Trader / non-trader declaration completed in the account settings.
- [ ] `PRIVACY.md` pushed so the privacy policy URL resolves.
- [ ] Version raised in both `manifest.json` and `package.json`.
- [ ] `bun run package` passed and the ZIP was loaded unpacked once in Chrome.
- [ ] Checked in a real call: in-call detection, recording, transcription (phrases still unverified) and Gemini notes.
