# Claudex

Claudex は Claude Code を操作画面とオーケストレーターとして使いながら、Codex、Grok
Build、Qwen Code、Claude の各モデルへ仕事を振り分けるローカル実行環境です。provider の利用率、
モデル、実行方式、fallback は
[`providers.json`](./providers.json) で一元管理します。
advisor はClaude Code標準の引数なし `advisor()` ツールを使用し、モデルは
[`settings.json`](../../.claude/settings.json) の `advisorModel` で管理します。

このREADMEは日常利用と別のMacへの導入手順を扱います。Anthropic Messages API互換
adapterの内部実装や開発上の詳細は
[`tools/claudex-agent-adapter/README.md`](../../tools/claudex-agent-adapter/README.md)
を参照してください。

## 現在の構成

```mermaid
flowchart LR
    User[ユーザー] --> Fish[fish: claudex]
    Fish --> Adapter[claudex-agent-adapter]
    Adapter --> Orchestrator[Claude main session]
    Orchestrator --> Hook[provider利用状況フック]
    Hook --> Codex[claudex-gpt\nCodex app-server]
    Hook --> Grok[claudex-grok\nGrok ACP]
    Hook --> Qwen[claudex-qwen\nQwen Code ACP]
    Hook --> Fallback[claudex-sonnet\nClaude fallback]
    Orchestrator -. 標準機能 .-> Advisor[Claude Code advisor()\nadvisorModel: opus]
```

現在の既定値は次のとおりです。

| 役割 | Agent | Model | Effort | 選択条件 |
| --- | --- | --- | --- | --- |
| Orchestrator | 通常のmain session | Claude Code設定 (`sonnet`) | Claude Code設定 (`xhigh`) | 通常起動 |
| Codex worker | `claudex-gpt` | `gpt-5.6-sol` | `high` | Codexに空きがある場合 |
| Grok worker | `claudex-grok` | `grok-4.5` | `high` | Grokに空きがある場合 |
| Qwen worker | `claudex-qwen` | `qwen3.8-max-preview` | `high` | Qwen Cloud quotaまたはcompatible APIが利用可能な場合 |
| Fallback | `claudex-sonnet` | `claude-sonnet-5` | `high` | 利用率を管理するproviderをすべて利用できない場合 |
| Advisor | Claude Code標準 `advisor()` | `opus` | Claude Code標準 | 標準advisor policyに従う |

worker のAgent定義と `providers.json` の両方に同じ固定モデルを指定します。adapterは
呼び出し時の `claudex_model` を最終的なprovider routeとして扱い、テストでfrontmatterと
共有設定の不一致を検出します。

Qwen ACPは `/usr/bin/env` 経由でQwen Codeだけに
`QWEN_WEB_FETCH_PROCESSING_TIMEOUT_MS=15000` を渡し、`--approval-mode yolo` でmain sessionの
無確認実行と同等のtool権限にします。Qwen Codeの `web_fetch` は取得後の
content processingを既定で最大300秒待つため、ここでは15秒でraw contentへfallbackさせます。
workerにも1 batch 1件・1 task 2件までの取得上限と同一URLの再試行禁止を指定し、複数URLの
逐次処理によってSubAgentが長時間応答しない状態を抑えます。

全SubAgent定義は `tools`、`disallowedTools`、`permissionMode` を省略し、main sessionの全toolと
permission contextを継承します。調査・reviewという役割だけを理由に読み取り専用へ変更しません。
background SubAgentはClaude Codeの仕様上、main sessionで対話確認できる未承認操作を自動拒否
するため、その可能性がある作業はforegroundで委譲します。main sessionを
`--dangerously-skip-permissions` で起動した場合、そのmodeはSubAgentにも優先して継承されます。
Grok ACPは `--always-approve`、Qwen ACPは `--approval-mode yolo` を明示し、provider自身の
approval待機やauto classifierがSubAgentの権限を狭めないようにします。

## ルーティング

1. `claudex` はClaude Code設定のモデルとeffortを継承した通常のmain sessionを起動し、
   claudex実行時だけglobal hookでorchestration contextを追加します。`mainProvider` は
   adapterのbootstrap routeとworker設定に使われ、通常起動のouter sessionへは強制しません。
2. prompt送信時にCodex/Grokは `codexbar usage --json` を使います。QwenはQwen Cloudの
   5時間・7日quotaを取得し、成功時刻から1時間未満はlocal cacheを再利用します。routing結果
   全体は既定で5分間キャッシュされます。
3. 各providerをquota windowの最大使用率が低い順、つまり最小残量の制約に最も余裕がある順に
   並べます。Qwen quota更新に失敗した場合は、Qwen Codeに保存済みのAPI keyを使う非生成の
   compatible `GET /models` で利用可能性を確認します。利用可能でも残量不明なら、既知の残量を
   持つproviderの後に置きます。片方のusage sourceが失敗しても別providerは無効化しません。
4. mainまたはworkerがAgent/Taskを起動するたび、そのturnへ注入された
   `selected_workers` からAgentを選び、model/effortを明示します。nested起動でもgeneric
   `claude`へのdefaultや親providerの無条件継承は行いません。
5. promptに `gpt...`、`grok...` または `qwen...` の完全なモデルIDがある場合は、
   `modelPrefixes` が一致するproviderへそのIDをそのまま渡します。ただし、端末固有の
   `CLAUDEX_DISABLED_SUBAGENT_MODELS` に含まれる完全一致モデルは明示指定でも拒否します。
6. providerを利用できない場合はClaude subscriptionのfallbackを使います。
7. 標準advisorはworkerの代替ではありません。provider quotaとは独立して動作し、会話履歴
   全体を自動参照します。

生response、アカウント情報、Cookie、API keyはキャッシュしません。
`~/.cache/claudex/usage-routing.json` にはrouting結果を5分間、
`~/.cache/claudex/qwen-quota.json` にはQwenのsanitized utilization、reset時刻、取得日時を
UTC ISO 8601形式の `fetched_at` として保存します。cache参照のたびにこの日時を読み、取得から
1時間未満なら再利用し、1時間以上なら更新します。いずれもモード `0600` で保存し、後者に
認証情報は含まれません。

### SubAgentの再利用

必要な並列性、役割分離、独立レビューのためのSubAgentを固定上限で抑制せず、作業に
必要な数を起動します。一方、1つの作業が終わっただけでは同じinstanceを自動的に破棄せず、
関連する追作業が見込まれ、agent、model、effort、scopeが互換なら、Agent/Task結果が指定した
正確な `SendMessage` recipient（通常agent ID、named mailbox teammateではteammate名）へ継続
します。追送は、そのrecipientが未確認の新しい証拠を含む、必要最小限で自己完結した差分にし、
会話contextとprompt prefixを再利用します。

独立した第二意見、clean-room review、真の並列実行、route/model/effortや権限範囲の変更では
新しいinstanceを起動します。終了時は、追作業とcache再利用の可能性に対して、slot・resource
圧力、contextの陳腐化や混入、役割の完了度を比較します。recipientは現在のmain session内
だけで扱い、推測・memoryへの永続化・TaskListによる再探索は行いません。

adapter daemon/backendの再利用とSubAgent会話instanceの再利用は別の層です。adapter側の
provider threadは通常2時間保持し、capacity到達時は最古のidle sessionを先に解放します。
完了済みagentを無意味に稼働させ続けるのではなく、logical recipientを保持して必要時に
resumeします。実際のprompt cache hitはprovider依存であり保証されません。

## 別のMacへの導入

### 1. 前提ソフトウェア

macOS 14以降を前提とします。先にHomebrewと次のコマンドを用意してください。

```sh
xcode-select -p >/dev/null || xcode-select --install
brew install fish rustup python uv jq
brew install --cask claude-code codex codexbar
```

- Claude Codeは[公式Quickstart](https://code.claude.com/docs/en/quickstart)に従って
  `claude` を起動し、ログインします。
- Codex CLIは[公式README](https://github.com/openai/codex#quickstart)に従って
  `codex` を起動し、ChatGPTまたはAPI keyでログインします。
- CodexBarを一度起動し、Settings → ProvidersでCodexとGrokを有効にします。
  詳細は[CodexBar README](https://github.com/steipete/CodexBar#install)を参照してください。
  `codexbar` コマンドが見つからない場合は、同READMEのCLI tarballまたはCLI install手順も
  実行してください。
- Grok Build CLIは利用可能な配布元からインストールし、`grok login` を実行します。
  このadapterは `grok --model MODEL agent --always-approve stdio` のACP接続を使用します。
  GrokはClaude互換hookのstdinを閉じないため、adapterはchildに `CLAUDEX_GROK_ACP=1` を渡し、
  `SessionStart` のClaude専用Herdr通知を入力読取前にskipします。これにより各sessionの10秒timeoutと
  timeout後に残るhook processを防ぎます。
- Qwen Codeは `bun add -g @qwen-code/qwen-code` など公式手順でインストールし、`qwen` の
  `/auth` からToken Planを設定します。API keyはclaudexへ重複設定せず、Qwen Code自身の
  設定を `qwen --acp --approval-mode yolo --model MODEL` が再利用します。

Qwen Cloudのremaining取得には、Chrome DevToolsのNetworkでToken Plan usage requestを
「Copy as cURL (bash)」し、repository localの `tmp/curl.txt` に保存します。このファイルは
login Cookieを含むためgit管理せず、ownerだけが読めるようにします。

```sh
chmod 600 tmp/curl.txt
```

別の場所へ保存する場合は `CLAUDEX_QWEN_QUOTA_CURL_FILE` に絶対pathを指定します。Cookieが
期限切れになった場合は新しいCopy-as-cURLで置き換えます。更新に失敗してもQwen Codeの
`~/.qwen/settings.json` にあるcompatible API設定でavailabilityを確認するため、routing全体は
継続します。Copy-as-cURLをshellとして実行することはなく、許可したQwen endpoint、Cookie、
form dataだけを解析してshellを介さずrequestを再構成します。

インストールと認証を確認します。

```sh
fish --version
claude --version
codex --version
grok --version
qwen --version
codexbar usage --json | jq '[.[] | {provider, has_usage: (.usage != null)}]'
python3 .claude/skills/claudex-routing/scripts/route_usage.py --no-cache | jq .
```

CodexBarの出力に `codex` と `grok` が含まれ、それぞれ `has_usage: true` になることを
確認してください。片方だけ使う場合は、後述の設定で不要なproviderを無効化できます。

### 2. dotfilesとClaude Code定義を配置

```sh
git clone git@github.com:kkkaoru/dotfiles.git
cd dotfiles
./create-symlinks.sh
```

`create-symlinks.sh` はClaude Codeの履歴やruntimeディレクトリを上書きせず、管理対象の
Agent、Skill、Command、Hook、settingsだけを `~/.claude` へリンクします。また、
次のClaudex関連ファイルもリンクします。

- `~/.config/claudex` → `.config/claudex`
- `~/.config/fish/functions/claudex.fish` → repositoryのfish function
- `~/.claude/agents/` 配下の全定義
- `~/.claude/skills/claudex-routing`
- `~/.claude/CLAUDE.md`（共通のSubAgent・orchestration方針）
- `~/.claude/settings.json`

既存の通常ファイルやディレクトリと競合した場合、スクリプトは上書きせず `skip` を表示
します。内容を確認して退避または統合したあと、もう一度実行してください。

### 3. Rust adapterをビルドしてインストール

Homebrew版rustupの初回だけtoolchainを初期化し、新しいshellを開きます。

```sh
rustup-init
rustup toolchain install stable --component clippy,rustfmt
```

repository rootからrelease buildをインストールします。

```sh
cargo install --locked \
  --path tools/claudex-agent-adapter \
  --root "$HOME/.local" \
  --bin claudex-agent-adapter
```

`~/.local/bin` が `PATH` に含まれることを確認してください。このdotfilesのfish設定では
自動的に追加されます。

```sh
command -v claudex-agent-adapter
claudex-agent-adapter build-id
```

### 4. 設定とdaemonを確認

```sh
jq empty "$HOME/.config/claudex/providers.json"

claudex-agent-adapter ensure \
  --provider-config "$HOME/.config/claudex/providers.json"

curl --fail --silent http://127.0.0.1:8318/health | jq .
```

`status` が `ok` で、`backend_routes` にCodex、Grok、Qwenが含まれていれば準備完了です。
常設のlaunchd plistは不要です。`claudex` の起動時に互換性のあるdaemonを再利用し、
存在しなければloopbackの `127.0.0.1:8318` へ自動起動します。

## 使い方

### 通常起動

任意のrepositoryへ移動して実行します。

```fish
cd /path/to/project
claudex
```

通常起動では `--agent` を追加せず、`CLAUDEX_ACTIVE` が設定されたプロセスでのみglobal
`UserPromptSubmit` hookがrouting contextを注入します。このため新規・resumeのどちらでも
sessionの表示名をagent名へ変更しません。adapterの `--inherit-claude-model` を使うため、outer sessionは
`~/.claude/settings.json` の `model` と `effortLevel` を継承します。
launcherは起動元のcwdを予約済みcustom headerでloopback adapterへ渡すため、daemonや
provider設定がdotfilesにあってもCodex、Grok、Qwen、Sonnetの作業ディレクトリは実行元を維持します。
既存の `ANTHROPIC_CUSTOM_HEADERS` は保持され、予約済みheaderだけが安全な値へ置き換わります。

SubAgentへの委譲はsubstantiveな調査・実装・レビューに対する既定動作なので、promptごとに
繰り返し指定する必要はありません。Claude Codeの `N queued` はmain conversationの次turnへ
渡す入力数であり、human promptとbackground Agentの完了通知を含みます。workerの実行slot数や
`SendMessage` の配信待ち数ではありません。現在turnで結果が必要な並列作業はforeground Agent
呼び出しをまとめて行い、backgroundは結果を待たずに有用な作業を継続できる場合に限定します。

### Orchestratorのモデルを指定

```fish
CLAUDEX_MODEL=grok-4.5 claudex
CLAUDEX_MODEL=gpt-5.6-sol claudex
CLAUDEX_MODEL=qwen3.8-max-preview claudex
```

`CLAUDEX_MODEL` を明示した場合だけClaude Code設定の継承を無効化し、指定モデルをouter
sessionにも使います。指定値は `modelPrefixes` と照合され、設定にないprefixのモデルは
起動時に拒否されます。

### 作業workerのモデルをpromptで指定

```text
gpt-5.6-sol のworkerを使ってこの変更を実装してください。
```

Orchestratorは完全なモデルIDを `claudex_model` としてAgentへ渡し、一致するbackendを
遅延起動します。設定済みprefix内であれば、`defaultModel` 以外も同じ方式で選択できます。

### 端末ごとにSubAgentモデルを禁止

`providers.json` やAgent定義を削除せず、その端末から起動するClaudexのSubAgentだけで
完全一致モデルを禁止できます。main sessionのモデルと標準advisorには影響しません。

```fish
# この1回の起動だけCodex workerを禁止
env CLAUDEX_DISABLED_SUBAGENT_MODELS=gpt-5.6-sol claudex

# 現在のfish端末で複数モデルを禁止
set -gx CLAUDEX_DISABLED_SUBAGENT_MODELS gpt-5.6-sol,grok-4.5
claudex

# 現在の端末の禁止を解除
set -e CLAUDEX_DISABLED_SUBAGENT_MODELS
```

値はカンマ区切りの完全なモデルIDです。Claudex起動後に親shellの値を変更しても既存session
には反映されないため、変更後は新しい `claudex` sessionを開始してください。routing hookは
禁止モデルを候補・fallback・再利用から除外し、共有daemonも端末別request headerを使って
SubAgent provider実行直前に再検証します。全モデルが禁止または利用不能ならmain sessionが
作業を継続し、SubAgent routingが利用できないことを報告します。

### 標準Advisorを利用

```text
標準advisorを使って設計をレビューし、workerの実装結果と統合してください。
```

Claude Code標準の `advisor()` は引数を取らず、呼び出し時点の会話履歴全体を自動参照します。
`providers.json` ではmodel routingせず、`.claude/settings.json` の `advisorModel: opus` を
使用します。

### 非対話実行

引数はClaude Codeへそのまま転送され、routing hookは自動で有効になります。

```fish
claudex --print \
  '標準advisorを使って、この設計をレビューしてください。'
```

### 一時的な設定上書き

```fish
CLAUDEX_PROVIDER_CONFIG=/path/to/providers.json claudex
CLAUDEX_USAGE_CACHE_SECONDS=0 claudex
CLAUDEX_SUBSCRIPTION_MAX_PROCESSES=8 claudex
CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES=60 claudex
CLAUDEX_ADAPTER_LISTEN=127.0.0.1:9418 claudex
CLAUDEX_DISABLED_SUBAGENT_MODELS=gpt-5.6-sol,grok-4.5 claudex
```

`CLAUDEX_USAGE_CACHE_SECONDS=0` は調査時だけ使用してください。通常はprovider CLIへの
不要な問い合わせを避けるため、既定の5分キャッシュを推奨します。

## providerやACPの追加

### 既存providerのモデルを変更

`providers.json` の `defaultModel` を変更します。同じproviderで将来追加されるモデルを
動的に受け入れる場合は `modelPrefixes` を維持または追加します。

`defaultModel`、対応するworker frontmatter、呼び出し時の `claudex_model` を同じ値へ
更新してください。テストは共有設定とAgent定義の不一致を拒否します。

### providerを無効化

```json
{
  "id": "grok",
  "enabled": false
}
```

実際のobjectでは他の必須フィールドを残し、`enabled` だけを `false` にします。

### 汎用ACPを追加

Rustコードを変更せず、`configured-acp` providerを追加できます。

```json
{
  "id": "vendor",
  "agent": "claudex-vendor",
  "defaultModel": "vendor-model-1",
  "effort": "high",
  "enabled": true,
  "modelPrefixes": ["vendor-"],
  "backend": "configured-acp",
  "acp": {
    "program": "vendor-cli",
    "arguments": ["--model", "{model}", "agent", "stdio"]
  }
}
```

`arguments` はshellを介さず直接実行され、すべての `{model}` が選択モデルに置換されます。
`agent` と同名の `~/.claude/agents/claudex-vendor.md` も作成します。Claude Codeが外部model
IDを受理しないproviderではfrontmatterを `model: inherit` にし、呼び出し時の
`claudex_model` で固定します。利用率をCodexBarで管理するproviderには `usageProvider` を
追加します。Qwen Cloud quotaを使うproviderは `usageProvider: "qwen"` とします。quota更新に
失敗した場合は `defaultModel` と一致するQwen Codeのcompatible API設定をavailability確認に
使います。省略したproviderは常に利用可能なunmetered providerとして扱われます。

## 更新

別のMacでdotfilesを更新した場合は、リンクを再確認してadapterを再インストールします。

```sh
git pull --ff-only
./create-symlinks.sh
cargo install --locked --force \
  --path tools/claudex-agent-adapter \
  --root "$HOME/.local" \
  --bin claudex-agent-adapter
```

次回の `claudex` 起動時に、protocol、route、process limitが一致しない古いdaemonは
自動的に置き換えられます。

## 開発時の検証

### Rust adapter

```sh
cd tools/claudex-agent-adapter
cargo fmt-check
cargo lint
cargo test-all
cargo coverage
```

通常coverageは全体のline、function、regionと、各production fileのlineを95%以上に
保ちます。branch coverageにはnightlyと `cargo-llvm-cov` が必要です。

```sh
rustup toolchain install nightly --component llvm-tools-preview
cargo install cargo-llvm-cov --locked
cargo coverage-branch
```

### Routing hook

```sh
cd .claude/skills/claudex-routing
uv run tests/run_coverage.py
python3 scripts/route_usage.py --no-cache \
  | jq -e '.hookSpecificOutput.hookEventName == "UserPromptSubmit"'
```

Routing hookのstatement coverageとbranch coverageは、どちらも95%以上を必須とします。

## トラブルシューティング

### `provider config is not readable`

```sh
ls -ld "$HOME/.config/claudex"
ls -l "$HOME/.config/claudex/providers.json"
./create-symlinks.sh
```

### AgentまたはSkillが見つからない

```sh
ls -l "$HOME/.claude/agents/claudex-orchestrator.md"
ls -ld "$HOME/.claude/skills/claudex-routing"
./create-symlinks.sh
```

Claudexをdotfiles repository以外から使うには、AgentとSkillがproject-localではなく
`~/.claude` にリンクされている必要があります。

### providerがfallbackになる

```sh
codexbar usage --json | jq '[.[] | {provider, usage}]'
stat -f '%Sp %N' tmp/curl.txt
env CLAUDEX_USAGE_CACHE_SECONDS=0 \
  python3 "$HOME/.claude/skills/claudex-routing/scripts/route_usage.py" \
  | jq .
```

`qwen-quota.json` の取得時刻から1時間以上経過していればQwen quotaを更新します。更新に
失敗してもcompatible APIが利用可能ならQwenを残量不明として候補に残します。providerが
存在しない、quota windowが100%、またはusageとavailabilityの両方を確認できない場合は、その
providerだけを利用不可にします。すべて利用不可の場合にfallbackを選びます。

### daemon設定が古い

```sh
claudex-agent-adapter ensure \
  --provider-config "$HOME/.config/claudex/providers.json"
curl --fail --silent http://127.0.0.1:8318/health | jq .
```

`providers.json` のQwen起動引数を含むroute定義はdaemon互換性判定の対象です。timeout値を変更
した場合も `ensure` が旧daemonを置き換え、新しいQwen childへ設定を反映します。

外部のlaunchd jobなどが旧 `--backend-route` 引数で同じportをKeepAliveしていると、共有
設定のdaemonを置き換えてしまいます。その場合は該当jobを停止し、`--provider-config`
参照へ更新してください。
