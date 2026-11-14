# claude-plamo-translate

A Claude Code mod that translates between you and Claude with
[PLaMo 3 Translate 0.8B](https://huggingface.co/pfnet/plamo-3-translate-0.8b), running locally:
MLX 4-bit on `mlx_lm.server` on a Mac, the official GGUF on llama.cpp's `llama-server` (Vulkan, so
GeForce and Radeon alike) on Linux and Windows. Nothing leaves the machine.

## What it does

- **Prompts** you type (or send through Remote Control) are translated into `promptLanguage`
  (default `en`) before the model reads them. Slash commands, notifications and prompts already
  in that language pass through unchanged.
- **Prompt history**: Claude Code keeps the translation the model read, so an entry recalled with
  the up/down arrows (or the history search) would come back in English. A prompt this session
  sent is put back exactly as you typed it (the last 200 are remembered); an older one is
  translated into `replyLanguage`. Slash commands and your own draft are left alone, and nothing
  is filled once you have moved to another entry.
- **Every text block** Claude writes, the ones between tool calls included, and the
  AskUserQuestion dialog's questions and options are shown in `replyLanguage` (default `ja`).
  Option labels stay as written (Claude Code draws the original dialog when a mod relabels an
  option), so each translated label leads its option's description. Tool rows ("Ran …"), tool
  output and logs stay as written. Only the drawing changes: the model and the transcript keep the
  original (ctrl+o).
- **Resumed sessions** are not translated all at once. In the fullscreen terminal only the blocks
  on screen are translated, newest first; one scrolled into view is shown as written until its
  translation lands, then redrawn. At most four requests run at once, a new prompt's first.
- **Progress**: a reply or dialog translation still running after two seconds shows a toast
  (`PLaMo: translating… 12/30 lines`) for `toastSeconds` (default 5). A toast cannot be closed early, so a short one keeps it from
  lingering after the translation is done. A new one appears only once
  the last is gone, so a long run shows several, never overlapping. Prompt translations show none.

Code blocks, inline code and URLs are never sent for translation. If a translation drops one of
them, or the server is unreachable, the original text is used. While replies are translated, a
`plamo-translate:untranslated` section of the system prompt tells Claude so, and asks it to put
anything you must read or copy verbatim (commands, paths, identifiers, error messages) in them.

## Install

1. Clone this repository and set up the model (once per machine; the model is not in Git):

   ```sh
   ./scripts/setup-plamo-translate.sh   # Windows: run it in Git Bash
   ```

   - macOS: mlx-lm via uv; the model is downloaded and converted into
     `~/.local/share/plamo-translate`.
   - Linux and Windows: llama.cpp's pinned Vulkan build into
     `~/.local/share/plamo-translate/llama.cpp` and `plamo-3-translate-0.8b-Q4_K_M.gguf` beside
     it. A Vulkan driver is needed (the GPU vendor's driver on Windows, Mesa or the NVIDIA driver
     on Linux).

2. Register the checkout as the `dotfiles` marketplace and enable the mod. On a Mac that uses these
   dotfiles, `./create-symlinks.sh` registers the marketplace and the user settings already enable
   `plamo-translate@dotfiles`. Anywhere else:

   ```sh
   claude plugin marketplace add <checkout>
   claude plugin install plamo-translate@dotfiles --scope user
   ```

3. Start a new session, or run `/reload-plugins` in a running one.

When nothing answers on port 18555, the mod starts `~/.local/bin/mlx_lm.server` if it exists,
otherwise `llama-server` (`llama-server.exe` on Windows, under `%USERPROFILE%` when `HOME` is
unset).

## Options

Set in `/config` or `pluginConfigs["plamo-translate"].options`:

| Option | Default | Meaning |
| --- | --- | --- |
| `replyLanguage` | `ja` | Language Claude's text, questions and recalled prompts are shown in. |
| `promptLanguage` | `en` | Language your prompts are translated into for the model. |
| `port` | `18555` | Port of the local PLaMo server, started on demand. |
| `toastSeconds` | `5` | How long each progress toast stays up. |

Languages are PLaMo codes such as `ja`, `en` and `zh-hans`; `off` turns a direction off.

## Check

```sh
bun install
bun run check   # oxlint, tsc, vitest with coverage, claude plugin validate
```
