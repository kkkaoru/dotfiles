# Claudex

Claudex は Claude Code を操作画面とオーケストレーターとして使いながら、Codex、Grok
Build、Qwen Code、OpenCode Go、Claude の各モデルへ仕事を振り分けるローカル実行環境です。provider の利用率、
モデル、実行方式、fallback は
[`providers.json`](./providers.json) で一元管理します。
advisor は2系統を独立して併用します。Claude Code標準の引数なし `advisor()` は
[`settings.json`](../../.claude/settings.json) の `advisorModel` を使い、
custom-advisor SubAgent（`claude-fable-5` / `xhigh`）は worker capacity とは別管理の
論理的な session singleton として `SendMessage` で再利用します（hard process=1 ではない）。

このREADMEは日常利用と別のMacへの導入手順を扱います。Anthropic Messages API互換
adapterの内部実装や開発上の詳細は
[`tools/claudex-agent-adapter/README.md`](../../tools/claudex-agent-adapter/README.md)
を参照してください。

## 現在の構成

```mermaid
flowchart LR
    User[ユーザー] --> Fish[fish: claudex]
    Fish --> Adapter[claudex-agent-adapter]
    Adapter --> Orchestrator[Claude main session\nsettings.json model/effort]
    Orchestrator --> Hook[provider利用状況フック]
    Hook --> Codex[claudex-gpt\ngpt-5.6-luna\nCodex app-server]
    Hook --> CodexSpark[claudex-gpt-spark\ngpt-5.3-codex-spark\nCodex app-server]
    Hook --> Grok[claudex-grok\nGrok ACP]
    Hook --> Qwen[claudex-qwen\nQwen Code ACP]
    Hook --> DeepSeek[claudex-deepseek\nOpenCode Go ACP]
    Hook --> Fallback[claudex-sonnet\nClaude fallback]
    Orchestrator -. 標準機能 .-> BuiltinAdvisor[Claude Code advisor()\nadvisorModel: opus]
    Orchestrator -. 必要時に併用 .-> CustomAdvisor[custom-advisor\nclaude-fable-5 / xhigh]
```

現在の既定値は次のとおりです。

| 役割 | Agent | Model | Effort | 選択条件 |
| --- | --- | --- | --- | --- |
| Orchestrator | 通常のmain session | `sonnet[1m]` | `high` | `.claude/settings.json` を優先（adapterのbootstrap routeは `mainProviders` の空き状況で選択） |
| Codex worker | `claudex-gpt` | `gpt-5.6-luna` | `max` | Codexに空きがある場合 |
| Codex Spark worker | `claudex-gpt-spark` | `gpt-5.3-codex-spark` | `xhigh` | Codexに空きがある場合 |
| Fugu worker | `claudex-fugu` | `fugu` | `high` | CodexBarのSakana枠に空きがある場合 |
| Ollama GLM worker | `claudex-ollama-glm-5-2` | `glm-5.2:cloud` | `max` | CodexBarのOllama枠に空きがある場合 |
| Grok worker | `claudex-grok` | `grok-4.5` | `high` | Grokに空きがある場合 |
| Qwen worker | `claudex-qwen` | `qwen3.8-max-preview` | `high` | providerは維持するがSubAgentではdenylistにより禁止 |
| DeepSeek worker | `claudex-deepseek` | `opencode-go/deepseek-v4-flash` | `high` | CodexBarのOpenCode Go枠に空きがある場合 |
| Fallback | `claudex-sonnet` | `claude-sonnet-5` | `high` | 利用率を管理するproviderをすべて利用できない場合 |
| Built-in advisor | Claude Code標準 `advisor()` | `opus` | Claude Code標準 | 標準advisor policyに従う。provider capacity非依存 |
| Custom advisor | `custom-advisor` | `claude-fable-5` | `xhigh` | 明示指定時、または複雑・曖昧・高リスク・長期・停滞時。worker capacityとは別管理の論理 session singleton（hard process=1ではない） |

worker のAgent定義と `providers.json` の `subagentModel` に同じ固定モデルを指定します。
`defaultModel` はmain session用で、省略時はworkerにも使われます。adapterは
呼び出し時の `claudex_model` を最終的なprovider routeとして扱い、テストでfrontmatterと
共有設定の不一致を検出します。

Codex app-server用のcustom provider credentialは、daemon起動時の非空envを最優先します。
コピー対象の `model_providers` が宣言する `env_key` だけを対象に、欠落時は
`~/.codex/.env`、`~/.env` の順で補完します。無関係なdotenv変数やcredential値は
log・healthへ出力しません。credential変更後は、永続app-server childへ新しい起動環境を
渡すため共有daemonを再起動します。

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
approval待機やauto classifierがSubAgentの権限を狭めないようにします。OpenCode Go ACPは
`opencode acp` を起動し、モデルは adapter の `session/new` meta `modelId` で渡します
（CLIの `--model` は `acp` サブコマンドでは受け付けません）。既定モデルは
`opencode-go/deepseek-v4-flash` です。OpenCode内で実行されるprovider-owned toolはClaude側で
再実行しないようAnthropic `tool_use`へ変換せず、実行中だけthinkingの進捗として扱います。
このためClaude Codeの完了結果ではtool数が0に見える場合がありますが、OpenCode側では実行済みです。
DeepSeek workerは独立した調査をまとめて実行し、確定済みの判断を反復せず、長い処理のフェーズ間で
短い進捗を返すよう定義しています。

## ルーティング

1. `claudex` は `mainProviders` の順に利用可能なproviderを選び、その `defaultModel` でmain
   sessionを起動します。claudex実行時だけglobal hookでorchestration contextを追加します。
   `subagentModel` があるproviderではworkerだけを別モデルへ固定します。
2. prompt送信時にCodex/Grok/Sakana/Ollama/OpenCode Goは `codexbar usage --json` を使います。Ollamaの
   usage取得に失敗した場合はlocal Ollama APIのmodel catalogを確認し、対象modelが存在すれば
   残量不明の候補として維持します。QwenはQwen Cloudの
   5時間・7日quotaを取得し、成功時刻から1時間未満はlocal cacheを再利用します。routing結果
   全体は既定で5分間キャッシュされます。共有daemonの `/health` にあるmodel別
   `model_concurrency` はpromptごとに再取得し、usage cacheには保存しません。health URLは
   `CLAUDEX_DAEMON_HEALTH_URL`、loopback `ANTHROPIC_BASE_URL` のorigin、既定の
   `http://127.0.0.1:8318/health` の順に解決します。
3. 各providerをquota windowとmodel別並列上限のうち、より厳しい使用率が低い順に並べます。
   `maxConcurrency` に達したmodelはそのturnの候補から外します。Qwen quota更新に失敗した場合は、
   Qwen Codeに保存済みのAPI keyを使う非生成の
   compatible `GET /models` で利用可能性を確認します。利用可能でも残量不明なら、既知の残量を
   持つproviderの後に置きます。healthを取得できない場合はproviderを起動可能な候補として残し、
   adapter側のhard limitに最終判定を委ねます。片方のusage sourceが失敗しても別providerは
   無効化しません。
4. mainまたはworkerがAgent/Taskを起動するたび、そのturnへ注入された
   `selected_workers` からAgentを選び、model/effortを明示します。nested起動でもgeneric
   `claude`へのdefaultや親providerの無条件継承は行いません。親のmain modelと同じmodelが
   `selected_workers` に明示されている場合は、outer requestとは独立したSubAgentとして起動します。
5. promptに `gpt...`、`fugu...`、`glm-...`、`grok...` または `qwen...` の完全なモデルIDがある場合は、
   `modelPrefixes` が一致するproviderへそのIDをそのまま渡します。ただし、専用設定と
   端末固有の追加設定を統合したdeny listに含まれる完全一致モデルは明示指定でも拒否します。
6. providerを利用できない場合はClaude subscriptionのfallbackを使います。
7. advisorはworkerの代替ではありません。Claude Code標準の `advisor()` はprovider quotaと
   独立して会話履歴全体を自動参照します。`custom-advisor` もworker capacity /
   `selected_workers` スロットとは別管理で、実装を行わず戦略レビューとpeer `SendMessage`
   に使います。両者は置換関係ではなく併用可能です。

生response、アカウント情報、Cookie、API keyはキャッシュしません。
`~/.cache/claudex/usage-routing.json` にはrouting結果を5分間、
`~/.cache/claudex/qwen-quota.json` にはQwenのsanitized utilization、reset時刻、取得日時を
UTC ISO 8601形式の `fetched_at` として保存します。cache参照のたびにこの日時を読み、取得から
1時間未満なら再利用し、1時間以上なら更新します。いずれもモード `0600` で保存し、後者に
認証情報は含まれません。

### SubAgentとcustom-advisorの再利用

必要な並列性、役割分離、独立レビューのためのworker SubAgentを固定上限で抑制せず、作業に
必要な数を起動します。一方、1つの作業が終わっただけでは同じinstanceを自動的に破棄せず、
関連する追作業が見込まれ、agent、model、effort、scopeが互換なら、Agent/Task結果が指定した
正確な `SendMessage` recipient（通常agent ID、named mailbox teammateではteammate名）へ継続
します。追送は、そのrecipientが未確認の新しい証拠を含む、必要最小限で自己完結した差分にし、
会話contextとprompt prefixを再利用します。

`custom-advisor` は論理的なsession singletonとして同じ方針で再利用します。最初に起動した
互換instanceを関連判断の継続advisorとし、完了後も含めて `SendMessage` で再開します。
これはOS process数のhard cap（process=1）ではなく、session内の再利用方針です。worker
capacityとは別勘定であり、`selected_workers` の空きを消費しません。真の並列レビュー、
clean-room review、role/model/contextの非互換、recipient不可用の場合だけ別instanceを起動
します。workerやpeerは、作業方針を変えうる戦略的助言が必要なときに同じadvisorへ
`SendMessage` できます。

独立した第二意見、clean-room review、真の並列実行、route/model/effortや権限範囲の変更では
新しいworker instanceを起動します。終了時は、追作業とcache再利用の可能性に対して、
slot・resource圧力、contextの陳腐化や混入、役割の完了度を比較します。recipientは現在の
main session内だけで扱い、推測・memoryへの永続化・TaskListによる再探索は行いません。

adapter daemon/backendの再利用とSubAgent会話instanceの再利用は別の層です。adapter側の
provider threadは通常2時間保持し、capacity到達時は最古のidle sessionを先に解放します。
完了済みagentを無意味に稼働させ続けるのではなく、logical recipientを保持して必要時に
resumeします。Claude subscriptionを使うcustom-advisorは継続時も内部process自体は毎回新規
になり得ますが、同じlogical transcriptを渡すため再利用可能なprompt prefixを保てます。
実際のprompt cache hitはprovider依存であり保証されません。

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

`status` が `ok` で、`backend_routes` にCodex、Grok、Qwenが含まれ、上限を設定したmodelが
`model_concurrency` に `active`、`queued`、`limit`、`available` を持てば準備完了です。
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

### SubAgentの並列制御

`claudex` はClaude Codeのmain sessionへ次の環境変数をexportして、通常のSubAgentの
並列方針を一元化します。値は起動ごとに環境変数で上書きでき、`MAX_PARALLEL` は上限であって
常にその数を起動する指定ではありません。

| 環境変数 | 既定値 | 役割 |
| --- | ---: | --- |
| `CLAUDEX_SUBAGENT_MIN_PARALLEL` | `3` | 独立した実装・調査・検証workstreamを同じbatchで開始する最小数 |
| `CLAUDEX_SUBAGENT_MAX_PARALLEL` | `40` | 利用可能なworker slotに応じて動的に増やす上限（`CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS`にも反映） |
| `CLAUDEX_SUBAGENT_ACTIVE_FLOOR` | `2` | 通常workerの実行中数の下限。1件になった時は追加work、追指示、または安全な中断・再割当を再評価 |
| `CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES` | `2` | 同じphaseで選ぶmodel familyの最小種類数。利用可能なproviderが足りない場合は理由を通知 |
| `CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION` | `1` | workerの完了・失敗・timeoutごとに残作業、追指示、追加launchを再判定 |
| `CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS` | `600` | 10分ごとのactive set・capacity・model familyの再評価間隔 |
| `CLAUDEX_SUBAGENT_REUSE` | `1` | model、effort、role、scopeが互換な完了workerを`SendMessage`で再利用 |
| `CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT` | `1` | main session終了・cancel・error時にlaunch停止、childのcancel/wait/reapを要求 |
| `CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS` | `30000` | 同一modelのadmission待機を有限化し、期限超過時は明示的なエラーを返す |

設定例:

```fish
# 通常workerを最大12件まで、最低3件・2 model familyで運用
CLAUDEX_SUBAGENT_MAX_PARALLEL=12 \
CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES=2 \
claudex
```

独立workが存在し、2つ以上の互換slotがあるphaseでは最低3 workerを同じbackground batchで
起動します。1件の重いworkerだけをforegroundで待ち続けることはしません。workerが完了するたびに
残りのworkerの終了可否を判定し、必要なら実行中workerへ追加の自己完結した指示を送り、同じ内容の
補助workerまたは新しいworkstreamを空きslotへ追加します。10分tickでも同じ再評価を行い、activeが
1件ならactive floor 2を回復する処理を優先します。これはcustom-advisorには適用せず、custom-advisor
は独立した論理session singletonとして必要時に再利用します。

minimumやmodel familyを満たせない場合は、provider quota、denylist、model別concurrency、または
ユーザーの明示的な単一worker指定という具体的な理由をrouting summaryへ残します。制限を黙って
破って同じmodelの直列実行へフォールバックしません。

### outer model/effort の既定値を切り替える

`claudex` の outer session は、既定では `$HOME/.claude/settings.json` の `model` と
`effortLevel` を使います。settings の `sonnet[1m]` / `high` を維持したまま、adapter には
provider の bootstrap model（通常は `gpt-5.6-luna`）を渡すため、Sonnet の outer session から
subscription route へ委譲できます。

頻繁に切り替える値は、Git 管理外の `~/.config/claudex/defaults.local.json` に保存できます。
このファイルは `.config/claudex/.gitignore` で除外され、JSON 以外の内容は実行しません。
`source` は `settings`（省略時の既定）または `explicit` を指定します。

```json
{
  "version": 1,
  "source": "explicit",
  "model": "gpt-5.6-luna",
  "effort": "max"
}
```

`explicit` では `model` / `effort` を outer session に渡し、`settings` では両値を
`~/.claude/settings.json` から読み取って `--inherit-claude-model` で起動します。設定ファイルの
`source` が不正、JSON が壊れている、または settings に必要な値がない場合は、別のモデルへ
黙って切り替えず `claudex` を終了します。`CLAUDEX_DEFAULTS_SOURCE=explicit claudex` のような
一時指定も可能です。既存の `CLAUDEX_MODEL` は explicit mode を選び、`CLAUDEX_EFFORT` は
model と独立して effort を上書きします。

SubAgentへの委譲はsubstantiveな調査・実装・レビューに対する既定動作なので、promptごとに
繰り返し指定する必要はありません。Claude Codeの `N queued` はmain conversationの次turnへ
渡す入力数であり、human promptとbackground Agentの完了通知を含みます。workerの実行slot数や
`SendMessage` の配信待ち数ではありません。独立workerを複数起動する場合、処理時間が不明または
重たい可能性がある作業は `run_in_background: true` のbackground batchとして同じturnで起動し、
1つのslow workerがmain turnや完了済みpeerを塞がないようにします。foregroundは、次のmain操作
前に結果が必須な短いbounded work、またはユーザーが同期完了を明示した場合だけ使います。全結果を
集めるだけのforeground batchは使いません。background起動後は具体的な独立作業を直ちに開始するか、
短いuser-visible statusを返してturnを終了します。完了通知が次turnへ入ったら、slowest workerを
待たず到着済み結果を逐次統合し、hidden reasoningで待機しません。既に委譲promptに含まれる制約の
再送は行わず、busy workerへの配信待ちは追加の並列数として数えません。

foreground batchではClaude Codeのmain turn自体が最後のworkerの完了まで占有されます。その間に
先に届いた `<agent-message>` が表示されても、mainが次の判断へ進めないため、`✶ Philosophising…`
が停止に見えることがあります。これは重いworkerの処理時間にmainを従属させていたことが原因です。
background batchなら完了済みpeerの通知を保持したままmain turnを解放し、遅いworkerの完了後に
残りの結果を統合できます。
Claude Code 2.1系の `N background agents launched` は複数の標準Agent tool cardをまとめた
headerです。直後の各identity行または `↓ to manage` から個別workerを確認できます。headerだけが
見える場合も、これを `N queued` や直列実行とは解釈しません。十分なterminal表示領域でidentity行が
継続して欠落する場合は、adapterではなくClaude Code TUI側の表示問題として切り分けます。

### Orchestratorのモデルを指定

```fish
CLAUDEX_MODEL=grok-4.5 claudex
CLAUDEX_MODEL=gpt-5.3-codex-spark claudex
CLAUDEX_MODEL=qwen3.8-max-preview claudex
```

`CLAUDEX_MODEL` を明示した場合だけClaude Code設定の継承を無効化し、指定モデルをouter
sessionにも使います。指定値は `modelPrefixes` と照合され、設定にないprefixのモデルは
起動時に拒否されます。

### 作業workerのモデルをpromptで指定

```text
gpt-5.3-codex-spark のworkerを使ってこの変更を実装してください。
```

Orchestratorは完全なモデルIDを `claudex_model` としてAgentへ渡し、一致するbackendを
遅延起動します。nested Agent/Taskでも `selected_workers` の同一entryにあるagent/modelを
必ず明示し、model未指定時にparentやmain modelを暗黙継承しません。設定済みprefix内であれば、
active userが完全なmodel IDを指定した場合に限り `defaultModel` 以外も同じ方式で選択できます。
`selected_workers` にmainと同じmodelがある場合も、その明示指定を優先します。

### SubAgentモデルを禁止

provider設定とは分離した `~/.config/claudex/disabled-subagent-models.json` に、常に禁止する
完全一致モデルを定義します。repositoryでは現在 `qwen3.8-max-preview` を
SubAgentで禁止しています。

```json
{
  "version": 1,
  "disabledModels": ["qwen3.8-max-preview"]
}
```

main sessionのモデルと標準advisorには影響しません。端末ごとに別の専用ファイルを使う場合や、
その端末だけ禁止モデルを追加する場合は次のように指定します。

```fish
# この端末だけ別の専用ファイルを参照
set -gx CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG /path/to/terminal-policy.json
claudex

# 専用ファイルに加えて、この端末だけ複数モデルを追加で禁止
set -gx CLAUDEX_DISABLED_SUBAGENT_MODELS gpt-5.6,grok-4.5
claudex

# 端末固有の上書きと追加を解除
set -e CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG
set -e CLAUDEX_DISABLED_SUBAGENT_MODELS
```

`disabledModels` と追加環境変数は完全なモデルIDです。環境変数側はカンマ区切りです。
Claudex起動後に設定を変更しても既存sessionには反映されないため、変更後は新しい `claudex`
sessionを開始してください。routing hookは統合した禁止モデルを候補・fallback・再利用から
除外し、共有daemonも端末別request headerを使ってSubAgent provider実行直前に再検証します。
全モデルが禁止または利用不能ならmain sessionが作業を継続します。

### 端末別ポリシー: providers のローカル上書き

`qwen` をこの Mac だけ有効化し、他の Mac では無効化する場合は、`providers.json` を
デフォルトで `qwen` 無効にした状態で、端末ごとの上書きファイルを使います。

- 端末別上書きファイル: `~/.config/claudex/providers.$(hostname -s).local.json`
- Git 追跡対象から除外: `~/.config/claudex/.gitignore`
- 例: `~/.config/claudex/providers.kkk4oru.local.json`

```json
{
  "version": 1,
  "mainProviders": ["codex", "grok", "fugu", "ollama-glm-5-2", "qwen", "opencode-go"],
  "providers": [
    {
      "id": "qwen",
      "enabled": true
    }
  ]
}
```

上書きファイルが存在する場合のみ反映し、明示的に `CLAUDEX_PROVIDER_CONFIG` を設定した
場合はそちらを優先します。

```fish
set -gx CLAUDEX_PROVIDER_LOCAL_CONFIG "$HOME/.config/claudex/providers.$(hostname -s).local.json"
claudex
```

### 標準Advisorとcustom-advisorを利用

```text
標準advisorを使って設計をレビューし、workerの実装結果と統合してください。
```

```text
custom-advisorを併用して設計をレビューし、workerの実装結果と統合してください。
```

Claude Code標準の `advisor()` は引数を取らず、呼び出し時点の会話履歴全体を自動参照します。
`providers.json` ではmodel routingせず、`.claude/settings.json` の `advisorModel: opus` を
使用します。

`custom-advisor` はこれと独立したSubAgentで、`claude-fable-5` / `xhigh` を使い、実装はせず
意思決定・リスク・検証観点とpeer向けの簡潔な助言を返します。session内では最初の互換
instanceを継続利用し、worker capacityとは別管理です。無効化する場合は次を設定します。

```fish
# custom-advisor SubAgentのみ無効（標準advisor()は利用可能）
set -gx CLAUDEX_CUSTOM_ADVISOR 0
claudex
```

`CLAUDEX_CUSTOM_ADVISOR` が `0` / `false` / `off`（大文字小文字無視）のときだけ
custom-advisor起動をスキップします。未設定またはそれ以外の値では有効です。

### 非対話実行

引数はClaude Codeへそのまま転送され、routing hookは自動で有効になります。

```fish
claudex --print \
  '標準advisorを使って、この設計をレビューしてください。'
claudex --print \
  'custom-advisorを併用して、この設計をレビューしてください。'
```

### 一時的な設定上書き

```fish
CLAUDEX_PROVIDER_CONFIG=/path/to/providers.json claudex
CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG=/path/to/disabled-models.json claudex
CLAUDEX_USAGE_CACHE_SECONDS=0 claudex
CLAUDEX_SUBSCRIPTION_MAX_PROCESSES=20 claudex
CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES=120 claudex
CLAUDEX_ADAPTER_LISTEN=127.0.0.1:9418 claudex
CLAUDEX_DISABLED_SUBAGENT_MODELS=gpt-5.6,grok-4.5 claudex
CLAUDEX_CUSTOM_ADVISOR=0 claudex
CLAUDEX_DEFAULTS_SOURCE=settings claudex
CLAUDEX_EFFORT=high claudex
CLAUDEX_SUBAGENT_MIN_PARALLEL=3 claudex
CLAUDEX_SUBAGENT_MAX_PARALLEL=20 claudex
CLAUDEX_SUBAGENT_ACTIVE_FLOOR=2 claudex
CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES=2 claudex
CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS=600 claudex
CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION=1 claudex
CLAUDEX_SUBAGENT_REUSE=1 claudex
CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT=1 claudex
```

`claudex` は `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` と上記のSubAgent policyを既定値でexportし、
外部から指定した値を優先します。通常の `claude` は `.claude/CLAUDE.md` の同じ方針を使いますが、
fish functionを経由しない場合は必要な環境変数をshell側で設定してください。`claudex` はadapter側の
`subscription-max-processes` を既定で20、`subscription-timeout-minutes` を120に揃えます。実行中のsessionには
反映されないため、上限変更後は新しいsessionを起動してください。

`CLAUDEX_USAGE_CACHE_SECONDS=0` は調査時だけ使用してください。通常はprovider CLIへの
不要な問い合わせを避けるため、既定の5分キャッシュを推奨します。

## providerやACPの追加

### 既存providerのモデルを変更

main sessionのモデルは `providers.json` の `defaultModel`、workerのモデルは任意の
`subagentModel` を変更します。同じproviderで将来追加されるモデルを
動的に受け入れる場合は `modelPrefixes` を維持または追加します。

`subagentModel`（省略時は `defaultModel`）、対応するworker frontmatter、呼び出し時の
`claudex_model` を同じ値へ更新してください。テストは共有設定とAgent定義の不一致を拒否します。

`maxContextTokens` をproviderごとに設定すると、`request` の概算入力トークン数が上限に達した時点で
新しいCodexスレッドを先に開始し、`contextWindowExceeded` を事前回避できます。未設定は現行どおり
既定値なし（制御なし）として扱われます。`gpt-5.3-codex-spark` は、
2026-07-26 の実運用ログで約116k入力トークン時に上限へ到達したため、再構築時の
システム指示やtool schemaの余白を確保して `110000` を採用しています。
`fugu` はCodex catalogの1M context windowに合わせて
`1000000` を指定しています。いずれもproviderが実際の上限を先に返した場合は、
非streaming turnを新規threadで1回だけ自動再試行します。

`maxConcurrency` はpositive integerのmodel別並列上限です。`subagentModel`（省略時は
`defaultModel`）だけでなく、同じproviderの `modelPrefixes` に一致して動的生成された各exact
model routeにも同じ値を継承します。共有daemonは `/health` の `model_concurrency` にexact model
ごとの `active`、`queued`、`limit`、`available` を公開し、routing hookはquota headroomと
予約済みqueueを含む空きslotの小さい方を
使って候補を並べます。OpenCode GoはCodexBarの `opencodego` usageと
`maxConcurrency: 7` の両方で制御します。healthが一時的に読めない場合も起動候補は維持されますが、
adapter自身が上限を強制するため超過実行は許可されません。

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
追加します。必要ならmodel別上限としてpositive integerの `maxConcurrency` も追加します。
Qwen Cloud quotaを使うproviderは `usageProvider: "qwen"` とします。quota更新に
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

`providers.json` のQwen起動引数を含むroute定義とbuild IDはdaemon切替判定の対象です。
`ensure` はport単位で多重起動を排他し、旧listenerを解放して同じportへ新buildを
起動します。旧daemonが受付済みの応答はそのprocess上で完了するため、idle sessionの保持期限を
待たずに設定とバイナリを反映できます。timeout値を変更した場合も新しいQwen childへ反映します。

外部のlaunchd jobなどが旧 `--backend-route` 引数で同じportをKeepAliveしていると、共有
設定のdaemonを置き換えてしまいます。その場合は該当jobを停止し、`--provider-config`
参照へ更新してください。
