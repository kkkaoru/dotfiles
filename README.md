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

Grok config is managed the same way as Pi: only an allowlisted set of files
under `.grok/` is tracked. `create-symlinks.sh` merges those into `~/.grok`
and leaves sessions, auth, caches, and binaries on the machine.

oMLX is managed like Pi: `~/.omlx` is a symlink into this repository.
`model_settings.json` and `settings.json.example` are tracked. Model weights
under `.omlx/models/` are gitignored. After cloning:

```sh
./create-symlinks.sh
./scripts/setup-omlx.sh
```

That downloads `mlx-community/Qwen3.8-27B-4bit` and `incoai/Qwen3.8-27B-DFlash2`
and restarts oMLX. See `.omlx/README.md`. `create-symlinks.sh` also installs
`~/.local/bin/pi` (starts oMLX on demand) and an idle-stop LaunchAgent so
`omlx-server` does not stay resident after the model unloads.

DeepSWE/Pier eval leftovers (task images, job dirs) can be removed with:

```sh
./scripts/cleanup-deepswe-disk.sh
```

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


## Dependency security

JavaScript dependencies are pinned in each component's `package.json` and `bun.lock`.
Use Bun, keep manifests and lockfiles together, and run the component's README checks
plus `bun audit` after updates. Audit the root CLI bundle as well as every local tool:

```sh
for lock in bun.lock tools/*/bun.lock; do
  (cd "$(dirname "$lock")" && bun audit) || exit 1
done
```

An ordinary install can leave obsolete nested packages in an existing `node_modules`.
After updating overrides, use `bun install --force --frozen-lockfile` and verify the
actually resolved dependency. If obsolete packages remain, replace only that generated
installation with a fresh frozen-lockfile installation; never move runtime databases
or credentials. A clean lockfile audit alone does not inspect those leftover packages.

CCR's `better-sqlite3` dependency needs its native addon installed. If Bun blocks its
install script, review that package's script and explicitly approve only that package
with `bun pm trust better-sqlite3`; do not blanket-trust dependencies. Verify with an
in-memory SQLite query before starting CCR. No Executor credentials or service setup
are involved in dependency verification.

The vendored MotherDuck pipeline has a separate `uv.lock`; see its
[dependency checks](.agents/skills/motherduck-build-data-pipeline/references/dlt-dbt-motherduck-project/README.md).
Rust components can be checked with `cargo audit --file tools/<component>/Cargo.lock`.
After updating loaded Pi dependencies, run `/reload` or start a new session.

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
