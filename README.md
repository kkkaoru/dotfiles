# Setup with Dotfiles

## Claudex

Claude CodeからCodex、Grok、Claude fallback、advisorを動的に使い分ける環境の導入と
使い方は[Claudex README](.config/claudex/README.md)を参照してください。

## Create sym links

```sh
./create-symlinks.sh
```

This also creates untracked `.pi/agent/packages/` links so Pi can resolve the
`./packages/...` entries in `.pi/agent/settings.json` after a fresh clone.

## Node.js

Use nodenv from anyenv

### Setup anyenv

```
anyenv install --init
```

## Install nodenv

```
anyenv install nodenv
```


## SHELL

Use fish with oh-my-fish


### Setup oh-my-fish

[How to install the official](https://github.com/oh-my-fish/oh-my-fish#installation)

```
curl -L https://get.oh-my.fish | fish
```

## OpenCode Go: 中国ホストモデルの利用許可を無効化する

OpenCode Go の DeepSeek など、中国でホストされるモデルの利用許可は
ワークスペース単位で変更します。無効化しても OpenCode の認証情報やモデル一覧は
削除されません。対象モデルを実行すると、再び opt-in を求めるエラーになります。

1. OpenCode にログインした状態で、エラーに表示された workspace URL を開きます。
   URL は通常 `https://opencode.ai/workspace/<workspace-id>/go` の形式です。
2. 対象のワークスペースを確認します。
3. Go の設定画面で `中国でホストされているモデルを有効にする` をオフにします。
4. 設定が保存されたことを確認して画面を閉じます。
5. 実行中の DeepSeek の claudex/OpenCode 子プロセスがあれば終了し、新しいセッションで再実行します。

認証状態の確認:

```sh
opencode auth list
opencode models
```

無効化後の動作を確認する場合は、対象モデルを実行します。opt-in を求めるエラーが
返れば、ワークスペース設定が無効になっています。

```sh
opencode run \
  --model opencode-go/deepseek-v4-flash \
  --variant max \
  --format json \
  "Return exactly OK"
```

この設定を無効化すると、中国でホストされるモデルは利用できなくなります。
データ所在地や組織のコンプライアンス要件を確認してから変更してください。

参考:

- [OpenCode Go 公式ドキュメント](https://dev.opencode.ai/docs/de/go/)
- [OpenCode Providers 公式ドキュメント](https://dev.opencode.ai/docs/providers)
