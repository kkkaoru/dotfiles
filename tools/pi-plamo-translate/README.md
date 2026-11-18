# pi-plamo-translate

A pi extension that talks to the same local
[PLaMo 3 Translate 0.8B](https://huggingface.co/pfnet/plamo-3-translate-0.8b) server as
`tools/claude-plamo-translate`. Nothing leaves the machine.

You see Japanese. The agent reads and writes English.

- A prompt you type in Japanese is translated to `promptLanguage` (default `en`) before the model
  reads it. The transcript shows what you typed. Slash commands, `!` shell lines, and messages
  injected by extensions are not rewritten.
- A text longer than 24 lines is left as written: translating one would ask for hundreds of
  requests two lines at a time. Drawing never starts a translation either, so a redraw of a long
  transcript, a resume or a reload asks the model for nothing.
- Assistant text, and compaction summaries, are drawn in `replyLanguage` (default `ja`). The session
  keeps the English. When a session opens (`startup`, `/resume`, `/fork`, `/reload`), only the most
  recent 2 persisted entries are translated, and an entry longer than 12 lines is left in English,
  so history does not flood the translation server. Tool rows and tool output stay as written.
  While a reply is still streaming it is shown as written; the Japanese replaces it when the
  translation lands. Prose is translated one paragraph per request, and the lines a hard-wrapped
  paragraph was split into are joined first, so a sentence is never translated in halves.
- Code spans, fenced blocks, URLs and the inside of 「...」 are not sent for translation. If the
  server is down, the original text is used and a warning is shown once.
- A translation still running after two seconds reports itself in one chat line
  (`PLaMo: 翻訳中… 12/30`), updated once every five seconds and cleared when the run ends. The same
  count is in the footer.

A prompt recalled with the up arrow in the current session is still what you typed. After a reload,
editor history is the English stored in the session.

## Install

The model is set up once per machine (it is not in Git):

```sh
./scripts/setup-plamo-translate.sh
```

On a Mac that downloads `kkk4oru/plamo-3-translate-0.8b-mlx-4bit-fast`, an all-4-bit conversion of the
model: it reads 478 MB per decode step where PFN's own MLX-4bit release reads about 0.98 GB, so it is
roughly twice as fast, at a small accuracy cost. `scripts/bench-plamo-translate.py --port 18555`
measures it on a fixed sample set.

`./create-symlinks.sh` links this directory to `~/.pi/agent/packages/pi-plamo-translate`. Settings
load `./packages/pi-plamo-translate`. Start a new session, or run `/reload`.

Options live under `pi-plamo-translate` in `~/.pi/agent/settings.json`:

| Option           | Default | Meaning                                                               |
| ---------------- | ------- | --------------------------------------------------------------------- |
| `replyLanguage`  | `ja`    | Language replies and summaries are shown in. `off` disables it.       |
| `promptLanguage` | `en`    | Language your prompts are translated into. `off` disables it.         |
| `port`           | `18555` | Local PLaMo server, started on demand and shared with the Claude mod. |

Up to eight translations run at once, a new prompt's first: batching them is what keeps an Apple
Silicon GPU busy. `mlx_lm.server` batches them into one decode loop, so eight translations finish in
about a third of the time eight sequential ones take (measured: 107 ms vs 305 ms), at a per-request
latency of 70-100 ms instead of ~40 ms. `scripts/bench-plamo-translate.py --concurrency 8` measures
it on a fixed sample set.

Greedy completions are also cached on disk (`~/.cache/plamo-translate/`), keyed by the prompt, the
token limit, the stop sequence and the model's `config.json`. The mods re-translate the same text
constantly — a redrawn block, a scrolled screen, a resumed session — and a repeat is answered
byte-identically in under a millisecond instead of ~100 ms per line (a three-line block: 158 ms,
then 1.2 ms). Sampled requests and `"cache": false` bypass it; `GET /cache/stats` reports the hit
rate.

## Check

```sh
bun install
bun run check
```
