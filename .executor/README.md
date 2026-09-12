# Executor home

`~/.executor` は、このディレクトリへのシンボリックリンクです。
`create-symlinks.sh` がリンクを作成・更新します。

## Git 管理の境界

ルートの `.gitignore` は `.executor` 配下をデフォルトで全除外し、
この README だけを追跡対象にしています。新しいファイルも自動的に除外されます。
共有可能な設定を追加する場合は、秘密情報や端末固有の値がないことを確認してから
個別に許可リストへ追加してください。`git add -f` で実行時データを登録しないでください。

以下はローカルに保持し、Git には登録しません。

- `data.db` と WAL / SHM / ロック等の関連ファイル（認証・接続情報などを含む可能性あり）
- `daemon-*.json`、`server-control/`（プロセス制御情報）
- `analytics-id`、`logs/`、`cache/`
- その他の認証情報、履歴、バックアップ、新規生成ファイル

再現用の連携定義は `tools/executor-skills/integrations.json`、セットアップは
`scripts/setup-executor.sh` で管理します。認証情報を DB から書き出して追跡しないでください。
Git の clone だけではローカル DB や認証状態は復元されません。

## 既存環境の移行

既存の `~/.executor` が実ディレクトリの場合、リンク作成スクリプトは上書きせず停止します。
Executor Desktop と関連デーモンを終了し、必要なら Git 管理外に安全なバックアップを
作成してから、隠しファイルや DB 関連ファイルも含めて移してください。
移行先に DB がある場合は自動マージや上書きをしないでください。
移行後、`~/.executor` からこのディレクトリへのリンクを作成します。

## サービスの起動スコープと別 Mac への展開

Executor はデータディレクトリとは別に、起動スコープのパスで連携設定を区別します。
`scripts/executor` は自身の checkout をデフォルトの `EXECUTOR_SCOPE_DIR` にします。
`./scripts/setup-executor.sh --install-service` はそのスコープを公式サービスの
LaunchAgent に保存してから、連携を登録します（登録には別途承認が必要です）。
`.executor` をスコープにすると、同じ DB でも登録済みの連携が見えなくなります。
サービスの再インストール時も元のスコープを維持してください。
別 Mac のパスが異なる場合は DB コピーに頼らず、セットアップスクリプトで再登録します。
詳しい前提・リンク作成・認証・検証は
[`tools/executor-skills/SETUP.ja.md`](../tools/executor-skills/SETUP.ja.md) を参照してください。

確認はリポジトリルートで `./scripts/executor service status` と
`./scripts/executor tools integrations` を実行します。
稼働中の DB や WAL / SHM を移動・削除しないでください。
