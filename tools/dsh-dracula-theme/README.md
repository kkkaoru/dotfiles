# dsh-dracula-theme

[Dracula](https://draculatheme.com) palette for the DeepSeek Harness UI: one
bundle that skins both the browser client (`dsh web`) and the Desktop app.

## How it works

The bundle contributes one plugin row (`id: dracula-theme`) whose `apply()`
pushes a boot `style` row onto `webserver/index-inject`. Both clients consume
that table:

- `dsh web` renders the row into the served `index.html`.
- Desktop receives the same table over its boot IPC and appends it to
  `document.head` before the shell mounts.

The stylesheet replaces `--dsw-alias-*`, `--dsw-specific-*`, and `--shiki-*`
tokens on `html body` / `html body[data-ds-dark-theme]`. That selector
out-specifies ui-theme's own token sheets, which the browser injects after this
row, so no `!important` is needed except for the attribute-scoped macOS menu
fill. Only tokens change, so components, icons, and layout stay stock. The
light and dark preferences both show Dracula, which is why `color-scheme` is
pinned to dark.

## Install into a profile

The package is a bundle (`dsh.bundle.patch`), so `dsh plugin` links it and
appends it to the profile's bundle list:

```sh
dsh plugin --profile web add "$HOME/ghq/github.com/kkkaoru/dotfiles/tools/dsh-dracula-theme"
dsh plugin --profile desktop add "$HOME/ghq/github.com/kkkaoru/dotfiles/tools/dsh-dracula-theme"
```

Quit the Desktop app before touching its profile, then reopen it. `dsh web`
picks the layer up on the next boot. Remove with
`dsh plugin --profile <name> remove dsh-dracula-theme`.

## Check without booting

```sh
dsh --profile web --dump-config | grep -A2 'dsh-dracula-theme'   # shows the layer
node --input-type=module -e "import('./index.js').then(m => { const h = new Map(); m.apply({ on: (n, f) => h.set(n, f) }); const rows = []; h.get('webserver/index-inject')(rows); console.log(rows.length, rows[0].kind, rows[0].text.includes('#282a36')) })"
```

## Changing the palette

Edit the color constants at the top of `index.js`; every declaration derives
from them. Dracula's canonical values are background `#282a36`, current line
`#44475a`, foreground `#f8f8f2`, comment `#6272a4`, cyan `#8be9fd`, green
`#50fa7b`, orange `#ffb86c`, pink `#ff79c6`, purple `#bd93f9`, red `#ff5555`,
yellow `#f1fa8c`. The Host re-reads a linked checkout on the next request, so
a running `dsh web` picks up an edit after rebooting the profile; Desktop needs
an app restart.
