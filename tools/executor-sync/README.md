# Executor settings sync (Bun, MVP)

Cloudflare R2を保存先にし、各MacのExecutorで実行する設定同期です。
**同期用daemon・LaunchAgent・cron・Docker・Workerは追加しません。**
既存の `scripts/executor call|tools|resume` の前後に短命な同期コマンドを呼びます。

## 状態と対応範囲

`.executor/sync.json` は **`{"format":2}` のみ**です。アカウントID、バケット、
保存先、公開鍵、秘密鍵、有効化状態を含めません。これらは各MacのKeychainに保存します。
Gitの設定変更だけでは同期先を変更したり、有効化したりできません。
初回設定・ペアリング後も無効で、`enable` による接続検証が必要です。

- 対応：同梱Executor **1.6.8**、現在のMCP連携、接続、OAuthクライアント、保存済みポリシー。
- 認証：接続が参照する `file` / `keychain` の値のみ抽出。OAuthクライアント秘密鍵は
  現在のローカルランタイム既定の `file` ストアに対応。別providerは黙って省略せず失敗。
- インポート先の認証はExecutor標準の `file` providerに格納し、新しいローカルIDで参照。
  **Executor自身の認証ストアは平文ファイルです。** 転送・履歴はage暗号化、ファイルは0600。
- 対象外：OpenAPI/GraphQL plugin、1Password等の外部provider、他tenant、実行履歴、
  ログ、進行中のOAuth/承認要求、MCPの実行中セッション。未対応pluginを検出したら全体を停止。
- 標準の秘密情報エクスポートAPIがないため、バージョン固定の専用アダプターを使用。
  公式ソース確認先：`RhysSullivan/executor@f1d95f2b657316180992d5a67c24b7b76dc2b0f1` の
  `apps/local/src/db/data-dir-ownership.ts`、`packages/core/sdk/src/core-schema.ts`、
  `packages/plugins/file-secrets/src/index.ts`、`packages/plugins/keychain/src/provider.ts`。

## R2の定義

| 項目 | 値 |
|---|---|
| バケット | `executor-config-sync`（非公開、Standard、default jurisdiction） |
| 最新設定 | `v1/personal/latest.age` |
| 履歴 | `v1/personal/history/<UTC日時>-<device UUID>-<revision UUID>.age` |
| 履歴保持 | `r2-lifecycle.json` のルールで30日 |
| 最新設定の期限 | なし |
| 認証 | Executor管理の既存Cloudflare OAuth接続（`cloudflare-api.user.default`） |
| 通信 | Executor → 公式Cloudflare MCP → R2オブジェクトREST API、HTTPS |
| 保存形式 | age暗号文をbase64化し、共有HMAC-SHA256で認証したJSON |
| 対象情報・鍵 | macOS Keychain、service `dotfiles.executor-sync` / account `profile-v2` |

`r2-lifecycle.json` はCloudflare REST APIの `/accounts/{account_id}/r2/buckets/executor-config-sync/lifecycle`
に渡す定義です。既存の無関係なルールは上書きせずマージしてください。
R2の公開URL・カスタムドメイン・公開CORS設定、S3キー、新規Workerは不要です。
ExecutorのOAuth接続を使うため、別Macでも初回にはこの接続を準備してください。
同期がその認証を取得するために同じ同期を必要とする循環を避けるためです。
従来のS3用Storageは互換テスト用に残っていますが、CLIはMCP経路のみ使用します。

## 初回設定（各Mac、一度のみ）

前提：dotfiles配置済み、`~/.executor` がその `.executor` へのリンク、
`setup-executor.sh --install-service` で既存Executorサービスを設定済み、Bun / bunx / ctx / age。
新しいMacでも、まずExecutorで空のDBを初期化します。同期ツールは空DBの新規生成をしません。

```sh
# ageがなければ brew install age
./scripts/setup-executor-sync.sh install
./scripts/setup-executor-sync.sh configure
./scripts/setup-executor-sync.sh enable
./scripts/executor-sync sync
```

最初のMacの `configure` はaccount IDだけを端末から非表示入力し、Keychainへ保存します。
age秘密鍵と独立した256-bit HMAC鍵はローカル生成します。既存プロファイルは上書きしません。
**別Macでは `configure` で別の共有鍵を生成せず、下記のペアリングを使ってください。**
秘密鍵はageの別FDへ渡します。平文スナップショットや鍵の一時ファイルは作りません。

### 別Macへの安全なペアリング

1. 両MacにExecutorと依存を準備し、対象アカウントのCloudflare OAuth接続を設定します。
2. 新しいMacで `./scripts/executor-sync pair-init`。そのMac専用の秘密鍵はKeychainに保存され、
   公開recipientだけが表示されます。
3. recipientを対面などの認証済み経路で照合します。共有元Macで
   `./scripts/executor-sync pair-export '<確認したrecipient>' /安全な場所/pair.age`。
4. 出力されたSHA-256を、ファイル転送とは独立した認証済み経路で新しいMacへ伝えます。
   暗号化ファイルだけを転送し、
   `./scripts/executor-sync pair-import /安全な場所/pair.age '<確認したSHA-256>'` を実行します。
   公開recipientだけでは送信元の本人性を保証できないため、digestの照合は必須です。
5. `enable` → `sync`。新しいMacでも保存先・共有鍵・有効化状態はKeychain内だけです。
   転送ファイルは不要になったら削除します。鍵をチャットやGitへ貼らないでください。

鍵を紛失すると復号できません。少なくとも1台の信頼できるMacと、必要に応じて安全な
暗号化バックアップを保管してください。ペアリングで新Macには共有データの読書き権限を
与えます。端末失効・共有鍵ローテーションは自動化していません。

`enable` はR2へ接続し、既存の最新設定があれば復号まで確認します。
初回同期ではR2に最新設定があればそれを優先し、なければ現在の設定をアップロードします。
初回にどちらを共有元にするかは、**設定が揃ったMacで先に初回同期するだけ**です。
日常運用の端末切り替え・使用権取得は不要です。

## 同期動作（常駐なし）

- CLI実行前：前回成功から30秒以上経過していれば取得・反映。
- CLI実行後：ローカル設定・トークンが変わっていればアップロード。
- Desktop UIだけの変更、長時間実行中のトークン更新、CLIを使っていないMacへの反映は、
  次のCLI実行か `executor-sync sync` まで待ちます。30秒ごとの常駐ポーリングではありません。
- 別の起動スコープ、Executorの `daemon/service/mcp/web` 等のライフサイクル操作、同期自身の
  CLI呼び出しにはフックを掛けません。標準出力と本来の終了コードを維持します。
- R2/Keychain障害でもフックはローカルのツール実行を妨げません。設定取込失敗は記録します。
- 同時変更はスナップショット全体で最後のR2書き込み優先。項目マージ・複数Macの排他制御なし。
  設定の削除も伝播します。変更を失った場合は暗号化履歴から復元できます。
- OAuthトークンの同時更新で失効した場合は再認証が必要です。
- 同期済みリモート設定は再送せず、ローカルID差による同期ループを防止します。
- 同じMacの同時同期だけをSQLiteのOSロックで抑止。プロセス終了時に自動解放されます。

## DBへの安全な反映

エクスポートは読み取りトランザクションで行い、設定4テーブルと参照される認証値だけを取得。
JSON列・OAuth有効期限のBLOBを保持し、tenant・row ID・実行時のhealth情報は同期しません。
ホーム・checkout・bun/bunx/ctxパスをポータブル表現に変換し、取り込み先で解決します。
未対応の絶対パスは反映しません。

**受信設定の反映時のみ既存Executorサービスを自動停止・再起動します。新しいdaemonは作りません。**
この間、別のエージェントが実行中なら呼び出しが中断される可能性があります。
既存のサービス登録を変更・削除せず、再起動が必要な状態をジャーナルに記録し、
途中終了した場合は次の同期で復旧します。

Executor本体と同じ `data.db.owner-lock` の排他ロックを取得できない場合、書き込みません。
スキーマ一致を確認し、取込前の設定を `sync-local/backups/*.age` に保存してから、
DBトランザクションで指定tenantの設定のみ入れ替えます。
認証値は新しいIDで追加するため、DBコミット前の異常終了でも既存認証を上書きしません。
古い未参照の認証値は自動削除しません。

ツール索引は同期せず、既存ローカル管理APIから再構築します。
再構築待ちは `sync-local/rebuild.json` に保持し、同期ごとに最大3接続ずつ処理します。
新しい接続がすぐ使えない場合は `sync` を再実行してください。
管理APIはloopback・正しいscopeだけに制限し、レスポンス・認証トークンはログへ出しません。

## 操作

```sh
./scripts/executor-sync status
./scripts/executor-sync doctor          # 読み取りのみ。設定/認証の件数だけ表示
./scripts/executor-sync sync
./scripts/executor-sync history
./scripts/executor-sync restore 'history/<一覧のID>.age'
./scripts/setup-executor-sync.sh disable
```

`history` は1000件に達した場合または継続カーソルがある場合、切り捨てず停止します。
R2への復元は旧版を新しい最新設定として公開します。
ローカル取込前バックアップの復元は今のMVPでは自動CLI化していません。
`sync-local/` 全体はGit除外対象です。ローカルバックアップは自動削除しません。

## セキュリティ境界と制限

- age暗号化だけでは送信元を認証できません。共有HMAC鍵による検証を**復号・取込前**に行い、
  MACをアカウント・バケット・グループにも結び付けます。R2への書込権だけでは設定を偽造できません。
- 既知の最新スナップショットの時刻をKeychainに保持し、それより古い最新データや、既知の
  最新データの消失を拒否します。古い履歴の復元は明示操作です。初回ペアリングは共有元の
  既知の時刻を引き継ぎます。完全な分散合意や、未観測の更新まで含めた巻き戻し防止ではありません。
- MCPの切り詰めを避け、読込は最大6000文字ずつ取得し、全体長・MACの一致を確認します。
  現在の暗号文上限は64 KiB（平文は安全余裕として60 KiB）。超過はアップロード前に拒否します。
- 通信先のMCP URLを公式エンドポイントに固定して検証します。エラー・承認待ち・ページング・
  不完全な応答を空データとして取り込みません。失敗時も通常のExecutor操作は利用できます。
- アカウントIDやバケット名の非公開化は補助策で、認証の代わりではありません。実際の防御は
  R2非公開、OAuth認可、暗号化、HMAC、Keychainによります。通信サービスには保存先が見えます。
- 信頼済みMac・OSユーザー・実行するdotfilesコードの侵害はこの方式では防げません。
  取り込む設定はコマンド実行や承認ポリシーを変えられるため、ペアリング相手とGit更新を信頼・検証してください。
- この変更だけで別Macの実機検証や同期の自動有効化が完了したことにはなりません。

## 検証

```sh
cd tools/executor-sync
bun run check
bun run smoke
```

単体テストは外部通信・暗号処理・Keychainをモックします。
オフラインsmokeは一時ディレクトリの合成DB2つ、合成トークン、その場限りのage鍵を使い、
暗号化→復号→別tenantへの認証・ポリシー取込と稼働中ownerロックの拒否を検証します。
実DB・実Keychain・R2・実サービスの停止には触れません。
