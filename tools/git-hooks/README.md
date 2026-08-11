# Config-based Git hooks

Git 2.54 以降の config-based hooks で、全 repository 共通の空白検証と、この
dotfiles の変更範囲に応じた品質検証を実行します。`core.hooksPath` や
`.git/hooks` へのコピーは使用しません。

Homebrew の Git（現在 2.55+）を PATH 先頭に置いてください。Apple Xcode Git
2.50 は `[hook]` を無視します。`.zshenv` / `.zprofile`（`/etc/zprofile` の
`path_helper` 後）と Fish の `brew shellenv`、および `create-symlinks.sh` の
`~/.local/bin/git` リンクで Homebrew Git を優先します。

`.gitconfig` では次の hook を定義しています。

- `staged-whitespace`: pre-commit で `git diff --cached --check`
- `dotfiles-pre-commit`: 変更対象の format、lint、構文検証、軽量テスト
- `dotfiles-pre-push`: 対象 project の全テストと 95% 以上の coverage gate

`dotfiles-git-quality` は実行専用の薄い shell launcher です。品質選択ロジックは
`quality_hook.py`、標準 `unittest` と statement/branch coverage は `tests/` に分離しています。
`create-symlinks.sh` が launcher を `~/.local/bin/dotfiles-git-quality` へリンクします。

品質チェックが失敗した場合、hook は非ゼロ終了し、commit / push を拒否します。

設定元と実行対象は次のコマンドで確認できます。

```sh
git --version   # 2.54+ required
git config --show-origin --get-regexp '^hook\.'
git hook list pre-commit
git hook list pre-push
git hook run pre-commit
```

特定 repository だけ一時的に無効化する場合は、定義を削除せず local config を使います。

```sh
git config --local hook.dotfiles-pre-push.enabled false
```
