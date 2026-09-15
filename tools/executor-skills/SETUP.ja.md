# pi + Executor のセットアップ

## 構成

```text
pi（既存 bash / tmux_exec + 案内 Skill 1件）
  └─ scripts/executor → Executor Desktop 同梱 CLI
       ├─ local-skills：既存 Skills の検索・本文・参照資料取得（読み取り専用）
       ├─ agmsg：公式スクリプト経由の明示的なメッセージ操作
       ├─ ctx：公式 `ctx mcp serve` によるローカル履歴検索
       ├─ Context7 / Chrome DevTools
       ├─ Cloudflare Docs / Agents SDK Docs
       ├─ Cloudflare API / Bindings / Builds / Observability / Browser / GraphQL
       └─ MotherDuck
```

pi-executor 拡張や別バージョンの Executor はインストールしません。
pi のネイティブ MCP 接続ではなく、既存の bash から Executor を利用します。
MCP の接続・ツール索引・資格情報・承認ポリシーは Executor に集約します。
Skills をクラウドへアップロードする構成ではありません。

## 認証・ポリシーまで共有する場合

下記は従来の再登録方式です。設定・OAuth認証・保存済みポリシーの同期を使う場合は
[`../executor-sync/README.md`](../executor-sync/README.md) を参照してください。
R2とage暗号化を使う任意のMVPで、初期状態は無効です。同期専用daemonは追加しません。
既存Executorサービスとローカル依存を準備してから同期を設定します。

## 別の macOS へのセットアップ

共有するのは **連携の再現用定義とスクリプト** です。`.executor/data.db`、
OAuth トークン、Keychain、ログ、履歴、承認ポリシーの実行時状態は共有しません。
Mac ごとに独立した Executor を起動し、OAuth は各 Mac で本人が認証します。
これは既存 DB の同期／エクスポート機能ではありません。

### 1. リポジトリと依存を用意

この変更と `scripts/`・`tools/executor-skills/` の関連ファイルをコミット・push してから、
別 Mac で clone / pull してください。ユーザー名や checkout パスは同じでなくて構いません。
Executor Desktop（`brew install --cask executor`）、Bun / bunx、jq、ctx CLI、
共通の `~/.agents/skills`（agmsg を含む）と、選択式の `~/.agents/skills-stroage` を用意します。
Executor の npm グローバル版は追加インストールしません。

### 2. Executor home をリンク

Executor Desktop を起動する前に、clone した dotfiles のルートで実行します。
これは Executor のリンクだけを作成し、他の dotfiles は変更しません。

```bash
# ~/.executor がまだ存在しない、新規 Mac の場合のみ
if [ ! -e "$HOME/.executor" ] && [ ! -L "$HOME/.executor" ]; then
  ln -s "$PWD/.executor" "$HOME/.executor"
else
  printf '%s\n' '既存の ~/.executor を確認してください。上書きしません。'
fi
```

すでに実ディレクトリがある場合は、Desktop と関連デーモンを停止し、安全な
バックアップを取ってから `.executor/README.md` の手順で移行します。
DB / WAL / SHM を稼働中に移動せず、移行先の DB とマージしないでください。
`./create-symlinks.sh` もリンクを管理しますが、他の設定も更新するため内容確認が必要です。

### 3. サービスと連携を再現

```bash
./scripts/setup-executor.sh --install-service
# 登録内容を確認済みで、今回の登録と認証不要接続の作成を承認する場合
./scripts/setup-executor.sh --approve-registration
```

初回の `--install-service` は公式 `executor service install --port 4789` を呼び、
この checkout のパスを `EXECUTOR_SCOPE_DIR` として LaunchAgent に保存します。
サービスのインストールは既存デーモンを再起動する可能性があります。
ポートを変更する場合は `--install-service --port 4790` のように指定します。
登録承認で停止した後の再実行には `--install-service` は不要です。
すべて確認済みなら両フラグを同時に指定することもできます。

`scripts/executor` はどの cwd から呼んでも自身の checkout を起動スコープに使います。
`~/.local/bin/executor` 経由でも同じです（明示的な `EXECUTOR_SCOPE_DIR` は優先）。
セットアップは常に自身の checkout を使い、Bun / bunx / ctx とローカル MCP のパスを
その Mac で解決します。`~/.executor` が正しいリンクでない場合、サービスの導入は停止します。

**スコープは重要です。** Executor 1.6.8 はスコープのパスからカタログの tenant を導出します。
データディレクトリを同じにしても、スコープが違うと既存登録が見えません。
既存 checkout の移動・改名は別 Mac の新規セットアップと異なり、既存 tenant の移行が必要です。
別の場所で安易に再登録したり、DB を直接編集しないでください。

### 4. 認証と動作確認

Cloudflare は下記 OAuth 手順、MotherDuck は接続画面から本人が認証します。
ポリシーは自動同期・常時許可化しません。端末ごとに UI で内容を確認してください。

```bash
./scripts/executor service status
./scripts/executor tools integrations
./scripts/executor tools search search_skills --namespace local-skills --limit 2
./scripts/executor tools describe local-skills.user.default.search_skills
./scripts/executor call local-skills user default search_skills '{"query":"cloudflare","limit":1}'
./scripts/executor web
```

上の呼出しは検索で同じパスが返った場合の例です。終了コードだけでなく `ok: true` と
内容を確認します。OAuth サービスのツール件数だけでは実際の認証成功を保証しません。
`executor web` の出力 URL はトークンを含む場合があるため、公開・コミットしないでください。

### 公式仕様と確認範囲

- [CLI](https://executor.sh/docs/local/cli)：サービス常駐、CLI の自動起動、MCP 接続。
- [Desktop](https://executor.sh/docs/local/desktop)：CLI と同じローカルサービスを共有。
- [Integrations](https://executor.sh/docs/concepts/integrations) /
  [Connections](https://executor.sh/docs/concepts/connections)：登録と接続・認証は別。
- [Policies](https://executor.sh/docs/concepts/policies)：許可・承認要求・拒否を分離。

公式ドキュメントは更新されます。現行ページの `tools sources` 等の例と、この Mac の
同梱 CLI **1.6.8** の `tools integrations` 等には差があり、スクリプトは実際の
`--help` と管理ツールの `tools describe` で検証した API を使用しています。
スコープ導出と LaunchAgent の環境保存は同梱ランタイムでも確認しました。
確認した公式ローカル手順には Mac 間の設定同期／秘密情報なしエクスポートの説明がなく、
未確認の同期機能には依存しません。Cloud / 自前ホストで一つのカタログを共有する方法は
別構成で、ローカル Skills や ctx も含めてそのままクラウドへ移すものではありません。

## 再セットアップ

前提：Homebrew cask の Executor、Bun、jq、ctx CLI、既存 Skills がインストール済み。

```bash
./scripts/setup-executor.sh
```

通常は Executor の承認で停止します。表示された登録内容を承認してから再実行します。
今回登録する MCP サーバー／接続を確認済みの場合に限り：

```bash
./scripts/setup-executor.sh --approve-registration
```

このフラグが承認するのはスクリプト内のサーバー登録・接続作成のみです。
ポリシーの常時許可化、既存の接続削除、クラウドのデータ更新はしません。
既存の登録・接続は再利用します（設定変更の上書き／同期はしません）。
`create-symlinks.sh` にも pi の案内 Skill パッケージを追加しています。

## Cloudflare の OAuth 認証

[Cloudflare 公式セットアップ手順](https://developers.cloudflare.com/agent-setup/prompt.md)
にある MCP 登録と OAuth ログインを、pi からは Executor 経由で行います。
ドメイン別 Skills は `~/.agents/skills-stroage` にあります。更新したセットアップでは、この保管庫も読み取り専用の `local-skills` カタログに含みます。既存の登録はファイル移動だけでは更新されません。
Codex/OpenCode 用の MCP 設定を別途追加したり、資格情報をコピーする必要はありません。

**MCP 登録だけでは OAuth は始まりません。** Executor 1.6.8 の管理画面から認証ページに
進めない場合でも、次のスクリプトで開始できます。

```bash
./scripts/executor-cloudflare-auth.sh cloudflare-api --approve-setup
# 用途別のサーバーも、それぞれ個別にログイン
./scripts/executor-cloudflare-auth.sh cloudflare-bindings --approve-setup
./scripts/executor-cloudflare-auth.sh cloudflare-builds --approve-setup
./scripts/executor-cloudflare-auth.sh cloudflare-observability --approve-setup
```

スクリプトは Executor の `oauth.probe` → `oauth.clients.registerDynamic` →
`oauth.start` を使い、返された Cloudflare 認証 URL を通常のブラウザで直接開きます。
動的クライアント登録で返された実際の ID を使い、コールバックは稼働中のローカル
Executor のポートを検出して統一します。アクセストークン、リフレッシュトークン、
PKCE verifier は Executor が管理し、スクリプトや Git には保存しません。

`--approve-setup` は今回の OAuth クライアント登録・認証開始に限った明示的な承認です。
省略時は Executor の確認が出たところで停止します。Cloudflare のログイン、
アカウント選択、アクセス権の承認は**本人がブラウザで**行ってください。
ポリシーの恒久的な許可やクラウドリソース変更は行いません。

ブラウザを開かず、用途別の6サーバーの OAuth クライアントだけ準備する場合：

```bash
./scripts/executor-cloudflare-auth.sh --all --prepare-only --approve-setup
```

`--all` で認証ページを一斉に開くことはできません。ログインはサーバーごとに開始します。
準備済みの OAuth クライアントは Executor に保存され、管理画面からも利用できます。
**ブラウザが開いたことは認証成功ではありません。** 承認後に `executor tools integrations`
で該当サービスのツール件数を、Executor の接続画面で状態を確認します。

```bash
executor web
```

MotherDuck は引き続き Integrations の接続追加から OAuth ログインを行います。

- `cloudflare-api`：DNS、Workers、R2、D1、KV、Zero Trust 等の API
- `cloudflare-bindings` / `cloudflare-builds` / `cloudflare-observability`
- `cloudflare-browser` / `cloudflare-graphql`
- `motherduck`：SQL、データ探索、Dive 等

これらは登録済みでも、認証するまでサービス用ツールが0件なのが正常です。
Docs、Agents SDK Docs、local-skills、Context7、Chrome DevTools は OAuth 不要です。
認証できたことと、個別のリソース変更を承認したことは別です。

## pi への反映

**新しい pi セッションを開始してください。** `/reload` でもリソースは再読込されますが、
既存の会話に含まれる長い一覧・取得結果は消えません。

共通の `~/.agents/skills` には Python/Rust/Swift/TypeScript 規約、Git commit、
find-skills、project-skills、agmsg/history の案内を置きます。agmsg/history は引き続き
Executor 経由で遅延読み込みし、案内 Skill は pi パッケージから読み込みます。
Cloudflare、MotherDuck、GPUI 等は `~/.agents/skills-stroage` に保管し、project-skills
で選んだものだけ対象プロジェクトの `.agents/skills` にコピーします。保管庫全体を
起動時探索に追加しないでください。プロジェクトで選んだ Skill まで隠れないよう、
旧ドメイン名のグローバル除外設定は削除しました。変更後は `/reload` または新セッションが必要です。

確認例：

```bash
executor tools search search_skills --namespace local-skills --limit 3
executor call local-skills user default search_skills '{"query":"cloudflare","limit":5}'
executor call local-skills user default read_skill '{"id":"cloudflare"}'
executor call local-skills user default read_reference \
  '{"id":"cloudflare","path":"references/workers/README.md","offset":0,"limit":24000}'
executor call cloudflare-docs user default search_cloudflare_documentation \
  '{"query":"Workers KV binding"}'
```

上記はこの端末の `user.default` 接続の例です。別環境では `tools search` が返す
正確なパスを使ってください。stdio サーバー登録時に Executor が自動作成する
`org.default` 接続が検索結果にも出る場合があります。両方を呼ぶ必要はありません。

## agmsg / ctx の実行

スキル本文は `local-skills`、実際の操作は別の `agmsg` / `ctx` integration を利用します。
ctx は公式 MCP サーバーをそのまま使用し、agmsg はこのパッケージの薄い MCP アダプターから
公式スクリプトだけを実行します。新しい常時許可ポリシーは追加しません。

```bash
executor tools search agmsg_whoami --namespace agmsg --limit 2
executor call agmsg user default agmsg_whoami '{"project":"/absolute/path/to/project"}'
executor tools search search --namespace ctx --limit 3
```

- 共有 Executor は pi の現在の送信者を推測できません。ルーターが pi の既存セッション情報から
  選択済みの名前・チームと元のプロジェクトを最小限の案内として渡し、`whoami` の結果と照合します。
  選択済み情報がなく複数候補がある場合は `/agmsg whoami` でユーザーに確認し、勝手に選びません。
- 送信・履歴・受信箱には `project` / `team` / `agent` が必要です。公式 `identities.sh` で
  そのプロジェクトの `pi` 登録を検証してから実行します。SQL・チーム設定への直接アクセスはしません。
- モデル用の直接 `agmsg` ツールは無効化しますが、既存 pi 拡張の自動受信とユーザー用
  `/agmsg` コマンドは維持します。自動受信を Executor 経由で二重にポーリングしません。
- ctx の検索には元のワークスペース／セッション条件を明示してください。既存インデックスを
  利用し、登録だけで再取り込みや履歴公開はしません。インデックスの不足は別途報告します。

## 検証・戻し方

```bash
cd tools/executor-skills
bun run tsc
bun run lint
bun run test
```

`bun run test` は Skills MCP・agmsg MCP・pi ルーティングのユニットテストと、Executor／ブラウザをモックした
OAuth スクリプトのオフラインテストを実行します。ライブ認証には触れません。

軽量化を戻すには `.pi/agent/settings.json` から今回の `skills` 除外配列を削除します。
案内 Skill も不要なら packages の `./packages/executor-skills` を削除します。
agmsg の直接ツールだけ戻すなら、その packages エントリーをオブジェクト形式にして
`{"source":"./packages/executor-skills","extensions":[]}` とし、pi を再読み込みします。
Executor のデータ／資格情報は `~/.executor` のリンク先（この checkout の `.executor`）に残り、Git 管理しません。
連携を削除する場合は Executor UI から個別に削除してください。

公式エンドポイントの出典は `integrations.json` の `sources` に記載しています。
