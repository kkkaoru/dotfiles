# Claude Code の実リクエスト記録と effort の対照実験

実施日: 2026-11-05。Claude Code 2.1.304、claude-opus-5-5、サブスクリプション認証(`oauth-2025-04-20`)。ローカルの記録用プロキシ(`ANTHROPIC_BASE_URL`、転送先は api.anthropic.com のみ)で、リクエストの構造と usage を記録した。認証ヘッダーは記録していない。`--setting-sources local` でユーザー設定を外し、`CLAUDE_CODE_PLUGIN_DIRS` も外したうえで、必要な plugin だけを `--plugin-dir` で読み込んだ。

出力 token 数は thinking を含む努力量の代理指標であり、回答の品質ではない。1 種類の課題、各条件 5 回、1 バージョンでの結果である。

## 1. /compact のリクエスト(直接観測)
- `compaction` パラメータも `compact-2026-09-04` beta も送られていない。**API の Compaction on demand ではない。** 送信 beta に `context-management-2025-06-27` があり、`context_management` は `clear_thinking_20251015` のみ。
- 通常の `/v1/messages` で、最後の user メッセージが `CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.` で始まる。model は同じ、`max_tokens` 128000、tools 12 個を維持。
- **effort はセッションの値がそのまま送られる。** `--effort low` では top-level も per-message も low、`--effort max` では max。
- 極小の会話での要約の出力 token: low 345、max 2,024(各 1 回)。

## 2. effort の送り方と cache(直接観測)
- Claude Code は top-level の `output_config.effort` と、`role: system` の per-message `output_config.effort` を両方送る。mod が step で effort を変えると、両方が変わる。beta は `per-turn-control-2026-07-01`。
- mod を使い、failing tool で medium → high → xhigh → high と変わった実行で、cache read は 20,069 → 20,207 → 20,328 → 20,417 → 20,513 と継続し、cache は維持された。

## 3. ターン途中の effort 変更は効くか(対照実験)
課題: Bash で date を実行した後、手計算で「1〜3000 のうち 7 で割り切れ、桁和が素数の整数の個数」を答える。tool_result の直後の step の出力 token を測定。実験用 plugin で step index ごとに effort を指定した。

| 条件 | 送信された top-level / per-message | 出力 token(昇順) | 中央値 |
|---|---|---|---|
| low → low | low / low | 2,936, 3,285, 4,506, 4,781, 5,994 | 4,506 |
| low → max(ターン途中) | max / low, max | 2,910, 3,170, 3,200, 5,241, 6,954 | 3,200 |
| max → max | max / max | 18,897, 19,889, 20,962, 21,428, 25,309 | 20,962 |
| max → low(ターン途中) | low / max, low | 16,210, 16,781, 19,986, 21,792, 22,107 | 19,986 |
| low → max(ターン途中、tool result に `context` を追加) | max / low, max | 3,440, 3,883, 3,951, 5,019, 6,612 | 3,951 |

新しいユーザーターンでの変更(同じ課題を tool なしで):

| 条件 | 出力 token(昇順) | 中央値 |
|---|---|---|
| low → low | 2,918, 3,132, 3,395, 4,444, 4,781 | 3,395 |
| low → max | 15,142, 17,619, 19,389, 21,173, 35,452 | 19,389 |

**読み取り:** ターン途中の変更は上げ下げどちらも、その step の出力量を変えなかった。新しいユーザーターンでは変わった。これは Effort ドキュメントの「tool_result だけの user ターンは新しい入力に数えない」という記述と一致する。top-level の値はターン途中でも max や low に変わっていたので、per-message の値が優先されていると見られるが、優先関係そのものは直接確認していない。

**回答:** 2 回目の実験(15 回)では 144 が 13 回、145 が 2 回で、low と low → max に 145 が出た。正答は検証していない。n が小さく品質の比較には使えない。

**無効とした実験:** 最初の実行(各 3 回)は hook の形式の誤りで effort が全条件 medium のままだったので、結果を使っていない。

## 4. autoCompactWindow の動作(実 mod、window 100,000)
`CLAUDE_CODE_AUTO_COMPACT_WINDOW=100000`、`--effort medium`、約 22k tokens のファイルを 1 メッセージ 1 回で読ませた。`/context` では window 200,000 のとき buffer 33k で、100,000 の閾値は 67k と計算される。

- **ターン途中:** コンテキストが約 65〜66k の次のリクエストで compaction。要約リクエストは約 66.8k〜69.5k、**effort は medium**、出力 945〜1,133 tokens。つまりエンジンの auto-compact は設定した window に従ってターン途中で発動する。
- **ターン終了時:** turn が 66,181 で `end_turn` した直後、mod の順序で compaction。要約リクエストは 68,217、**effort は max**、出力 2,658 tokens。mod の sequence は実機で動作した。
- **直後のサイズ:** 次のリクエストのコンテキストは 43,554〜43,921(読み直された 1 ファイルを含む)。別実行の完了後のターンでは 24,060。
- **閾値が低すぎる場合の挙動:** ターン途中の compaction の後でファイルの読み直しが起き、1 回の実行で compaction が 2〜3 回発生した。ファイルを順に読ませる課題の人工的な条件であり、頻度の推定には使えない。
- 実行後、dotfiles の作業ツリーに差分はなかった(`/effort` は `this session only` と表示)。
- settings 経由: `--settings` で `{"autoCompactWindow":200000}` を渡すと、`/context` は Tokens 19.4k / 200k、Autocompact buffer 33k を示した。環境変数 200,000 でも同じ表示。発動は settings 経由では未観測。

## 未確認
- 回答品質に effort がどう効くか。
- 1M window を維持して 167k などの高い閾値で同じ動作になるか(window 100k と 200k の `/context`、および 100k の発動のみ確認)。
- 約 54 分放置後の `auto` compaction がどの機能か。
- `context` 以外の方法(別の text block など)で tool loop 途中の変更を効かせられるか。
