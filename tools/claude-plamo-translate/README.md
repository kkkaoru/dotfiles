# claude-plamo-translate

A Claude Code mod that runs [PLaMo 3 Translate 0.8B](https://huggingface.co/pfnet/plamo-3-translate-0.8b)
locally: MLX 4-bit on `mlx_lm.server` on a Mac, the official GGUF on llama.cpp's `llama-server`
(Vulkan, so GeForce and Radeon alike) on Linux and Windows:

- **Prompts** you type (or send through Remote Control) are translated into `promptLanguage`
  (default `en`) before the model reads them. Slash commands, notifications and prompts already
  in that language pass through unchanged.
- **Every text block** Claude writes, the ones between tool calls included, and the
  AskUserQuestion dialog's questions and options are shown in `replyLanguage` (default `ja`).
  Option labels stay as written (Claude Code draws the original dialog when a mod relabels an
  option), so each translated label leads its option's description. Tool rows ("Ran …"), tool output and logs stay as written. Only the drawing changes: the
  model and the transcript keep the original (ctrl+o).

Code blocks, inline code and URLs are never sent for translation. If a translation drops one of
them, or the server is unreachable, the original text is used.

## Setup (once per machine)

```sh
./scripts/setup-plamo-translate.sh   # Windows: run it in Git Bash
```

- macOS: mlx-lm via uv; the model is downloaded and converted into `~/.local/share/plamo-translate`.
- Linux and Windows: llama.cpp's pinned Vulkan build into `~/.local/share/plamo-translate/llama.cpp`
  and `plamo-3-translate-0.8b-Q4_K_M.gguf` beside it. A Vulkan driver is needed (the GPU
  vendor's driver on Windows, Mesa or the NVIDIA driver on Linux).

The model is not stored in Git. When nothing answers on port 18555, the mod starts
`~/.local/bin/mlx_lm.server` if it exists, otherwise `llama-server` (`llama-server.exe` on Windows,
under `%USERPROFILE%` when `HOME` is unset). It is loaded as `plamo-translate@dotfiles`, enabled in
`.claude/settings.json` from the repository's `.claude-plugin/marketplace.json`.

## Options

Set in `/config` or `pluginConfigs["plamo-translate"].options`: `replyLanguage`,
`promptLanguage` (PLaMo codes such as `ja`, `en`, `zh-hans`; `off` turns a direction off) and
`port`.

## Check

```sh
bun install
bun run check   # oxlint, tsc, vitest with coverage, claude plugin validate
```
