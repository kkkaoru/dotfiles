# Config-based Git hooks

Git 2.54以降のconfig-based hooksで、全repository共通の空白検証と、このdotfilesの
変更範囲に応じた品質検証を実行します。`core.hooksPath` や `.git/hooks` へのコピーは
使用しません。

`.gitconfig` では次のhookを定義しています。

- `staged-whitespace`: pre-commitで `git diff --cached --check`
- `dotfiles-pre-commit`: 変更対象のformat、lint、構文検証、軽量テスト
- `dotfiles-pre-push`: 対象projectの全テストと95%以上のcoverage gate

`dotfiles-git-quality` は実行専用の薄いshell launcherです。品質選択ロジックは
`quality_hook.py`、標準 `unittest` とstatement/branch coverageは `tests/` に分離しています。
`create-symlinks.sh` がlauncherを `~/.local/bin/dotfiles-git-quality` へリンクします。

設定元と実行対象は次のコマンドで確認できます。

```sh
git config --show-origin --get-regexp '^hook\.'
git hook list pre-commit
git hook list pre-push
git hook run pre-commit
```

特定repositoryだけ一時的に無効化する場合は、定義を削除せずlocal configを使います。

```sh
git config --local hook.dotfiles-pre-push.enabled false
```
