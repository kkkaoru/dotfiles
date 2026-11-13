# claude-plamo-translate

A Claude Code mod that runs [PLaMo 3 Translate 0.8B](https://huggingface.co/pfnet/plamo-3-translate-0.8b)
(MLX 4-bit, `mlx_lm.server`) on this Mac:

- **Prompts** you type (or send through Remote Control) are translated into `promptLanguage`
  (default `en`) before the model reads them. Slash commands, notifications and prompts already
  in that language pass through unchanged.
- **Every text block** Claude writes, the ones between tool calls included, and the
  AskUserQuestion dialog's questions and options are shown in `replyLanguage` (default `ja`).
  Tool rows ("Ran …"), tool output and logs stay as written. Only the drawing changes: the
  model and the transcript keep the original (ctrl+o).

Code blocks, inline code and URLs are never sent for translation. If a translation drops one of
them, or the server is unreachable, the original text is used.

## Setup (once per Mac)

```sh
./scripts/setup-plamo-translate.sh   # mlx-lm via uv, download + convert to ~/.local/share/plamo-translate
```

The model is not stored in Git. The mod starts `~/.local/bin/mlx_lm.server` on port 18555 when
nothing answers there. It is loaded as `plamo-translate@dotfiles`, enabled in `.claude/settings.json` from the
repository's `.claude-plugin/marketplace.json`.

## Options

Set in `/config` or `pluginConfigs["plamo-translate"].options`: `replyLanguage`,
`promptLanguage` (PLaMo codes such as `ja`, `en`, `zh-hans`; `off` turns a direction off) and
`port`.

## Check

```sh
bun install
bun run check   # oxlint, tsc, vitest with coverage, claude plugin validate
```
