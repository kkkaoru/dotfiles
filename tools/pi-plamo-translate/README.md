# pi-plamo-translate

A pi extension that talks to the same local
[PLaMo 3 Translate 0.8B](https://huggingface.co/pfnet/plamo-3-translate-0.8b) server as
`tools/claude-plamo-translate`. Nothing leaves the machine.

You see Japanese. The agent reads and writes English.

- A prompt you type in Japanese is translated to `promptLanguage` (default `en`) before the model
  reads it. The transcript shows what you typed. Slash commands, `!` shell lines, and messages
  injected by extensions are not rewritten.
- Assistant text, and compaction summaries, are drawn in `replyLanguage` (default `ja`). The session
  keeps the English. Tool rows and tool output stay as written. While a reply is still streaming it
  is shown as written; the Japanese replaces it when the translation lands. Prose is translated two
  lines per request, so a long reply fills in as its pieces arrive instead of waiting for all of it.
- Code spans, fenced blocks, and URLs are not sent for translation. If the server is down, the
  original text is used and a warning is shown once.
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

`./create-symlinks.sh` links this directory to `~/.pi/agent/packages/pi-plamo-translate`. Settings
load `./packages/pi-plamo-translate`. Start a new session, or run `/reload`.

Options live under `pi-plamo-translate` in `~/.pi/agent/settings.json`:

| Option           | Default | Meaning                                                               |
| ---------------- | ------- | --------------------------------------------------------------------- |
| `replyLanguage`  | `ja`    | Language replies and summaries are shown in. `off` disables it.       |
| `promptLanguage` | `en`    | Language your prompts are translated into. `off` disables it.         |
| `port`           | `18555` | Local PLaMo server, started on demand and shared with the Claude mod. |

## Check

```sh
bun install
bun run check
```
