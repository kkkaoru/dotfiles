# pi + Executor のセットアップ

## 構成

```text
pi（既存 bash / tmux_exec + 案内 Skill 1件）
  └─ scripts/executor → Executor Desktop 同梱 CLI
       ├─ local-skills：既存 Skills の検索・本文・参照資料取得（読み取り専用）
       ├─ Context7 / Chrome DevTools
       ├─ Cloudflare Docs / Agents SDK Docs
       ├─ Cloudflare API / Bindings / Builds / Observability / Browser / GraphQL
       └─ MotherDuck
```

pi-executor 拡張や別バージョンの Executor はインストールしません。
pi のネイティブ MCP 接続ではなく、既存の bash から Executor を利用します。
MCP の接続・ツール索引・資格情報・承認ポリシーは Executor に集約します。
Skills をクラウドへアップロードする構成ではありません。

## 再セットアップ

前提：Homebrew cask の Executor、Bun、jq、既存 Skills がインストール済み。

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

## 本人が行う OAuth 認証

```bash
executor web
```

Executor の Integrations で、利用する連携を開き、接続の追加から OAuth ログインを
行ってください。アカウントと必要最小限の権限を選択します。

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

`.pi/agent/settings.json` の `skills` 除外設定は Cloudflare、MotherDuck、GPUI 等の
個別説明を起動プロンプトから外します。案内 Skill から Executor を検索し、必要な
Skill だけ読みます。既存ファイルは削除せず、Python/Rust/TypeScript 規約、Git commit、
agmsg、ctx、find-skills は引き続き通常読み込みです。他エージェントの設定も変更しません。

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

## 検証・戻し方

```bash
cd tools/executor-skills
bun run tsc
bun run lint
bun run test
```

軽量化を戻すには `.pi/agent/settings.json` から今回の `skills` 除外配列を削除します。
案内 Skill も不要なら packages の `./packages/executor-skills` を削除します。
Executor のデータ／資格情報はホーム側に残り、Git 管理しません。
連携を削除する場合は Executor UI から個別に削除してください。

公式エンドポイントの出典は `integrations.json` の `sources` に記載しています。
