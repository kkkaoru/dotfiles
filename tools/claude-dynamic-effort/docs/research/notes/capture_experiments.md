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

## 5. 採用した方式の実機確認(window 200,000)
effort の段階的な引き上げを廃止し、compact の間だけ max にする方式を実機で確認した。ターン途中の自動 compaction は、mod が skip して `$.turn.abort` でターンを止め、`/effort max`、`/compact`、`/effort 元の値`、続きのプロンプトの順にキューへ積む。リクエストの effort は書き換えず、シーケンス中に max を継承した step だけを元の値で送る。

| 条件 | 発動前の最後のリクエスト | 要約(max) | 直後のコンテキスト | 読み取り順 |
|---|---|---|---|---|
| 対話モード、2.1.304、`ENABLE_TOOL_SEARCH=true` | 173,621 | 出力 6,287、約 60 秒 | 65,622 | data1〜7、compact、data8〜9 |
| `-p`、stream-json | 176,284 | 出力 5,805、56 秒 | 45,283 | data1〜8、compact、data9 |

- どちらも `--settings` の `autoCompactWindow` で発動した。環境変数でなくても動く。
- 対話モードの終了時は `session=medium pending=false`。max で送られたのは compact のリクエストだけだった。
- 実行後、dotfiles の作業ツリーに差分はなかった。対話モードの `/effort medium` は `saved as your default` と表示するが、値は同じなので変わらない。
- **バックグラウンド処理(window 100,000、`-p`):** ターンを止めた後も、`sleep 110; date > bg_bash.txt` と、Agent の subagent が書く `bg_agent.txt` は最後まで完了した。
- **キューの順序:** 完全な先入れ先出しではない。バックグラウンドの完了通知と、compact 中に入力したプロンプトは、`/compact` と復元の間に実行された。このときのリクエストは medium で送られ、その後に復元が実行された(`session=medium pending=false`)。別の実行では、完了通知の後にプロセスを終了したため、復元の実行を確認できなかった。
- **閾値が低すぎる場合:** window 100,000 では、compact 後の続きのターンの終了時に 60k を超え、2 回目の compact が走った(出力 12,088、107 秒)。
- **検証の副作用:** `ANTHROPIC_BASE_URL` を使うと tool search が無効になり、ツール 115 個で開始時のコンテキストが約 122k になった。このとき compact 後も約 144k で、compact が 3 回連続した。`ENABLE_TOOL_SEARCH=true` ではツール 15 個、開始時約 40k で再現しなかった。`/opt/homebrew/bin/claude` は 2.1.295 で、この回はそれが起動していた。最終確認は `~/.local/bin/claude`(2.1.304)で行った。

## 6. 実セッションでの確認(2026-11-07、2.1.304、window 200,000)
いずれもローカルの transcript、バイナリ、再現実行から確認した。セッションの内容は記録しない。

### compaction の頻度
| 完了(JST) | trigger | 圧縮前 | postTokens | 所要(秒) | 直後の文脈 | 次までのリクエスト |
|---|---|---:|---:|---:|---:|---:|
| 16:58 | manual | 167,469 | 26,280 | 136 | 87,199 | 25 |
| 17:20 | manual | 168,274 | 28,371 | 166 | 95,730 | 30 |
| 17:27 | manual | 152,822 | 23,304 | 52 | 85,645 | 50 |
| 17:41 | manual | 170,264 | 31,175 | 193 | 94,365 | 13 |
| 17:52 | auto | 167,690 | 29,505 | 273 | 101,421 | 18 |
| 18:03 | auto | 170,415 | 32,051 | 281 | 107,098 | 16 |
| 18:18 | auto | 169,238 | 31,956 | 209 | 107,388 | - |

- 82 分に 7 回で、compaction の待ちは合計 1310 秒(約 27%)だった。間隔の中央値は約 13 分。
- **直接の原因は閾値と床の差である。** 直後の文脈は 85.6k〜107.4k で、閾値 167k まで 60k〜81k しか空かない。レポートの床 p50(71.6k)より高い。
- **増加量は異常ではない。** 1 リクエストあたり約 2.9k(外れ値の対を除く +444,778 / 151 間隔)。同時期に 1M window で動いていた 2 セッションは、compaction あたり 202 / 421 リクエストで、同じ計算で約 4k / 2k である。
- **外れ値の対:** 文脈が 1 リクエストだけ約 +134k〜154k になり、次で戻る対が 3 回あった。advisor の呼び出しで usage が合算された可能性がある(未確認)。直後に compaction は起きていない。

### 直後の文脈の内訳(6 回目。実送信の記録から)
先頭の固定部分(system と tools)は 30,508 token で全回共通である。セッション開始時の文脈は 48,212 だった。残りの書き込み 76,588 token の内訳は文字数で次のとおり(言語で 1 文字あたりの token が変わるので、token には換算しない)。

| 項目 | 文字数 |
|---|---:|
| 要約 | 35,571 |
| スキル一覧 | 28,724 |
| 指示ファイル | 25,882 |
| 呼び出し済みスキルの本文(2 件) | 24,187 |
| 復元ファイル(4 件) | 約 15,000 |
| SessionStart hook の出力 | 4,999 |
| 遅延ツール一覧、MCP の説明、エージェント種別、advisor の説明 | 12,819 |

- **要約は回を重ねると伸びた:** 10,775、15,290、8,781、17,698、28,507、35,571 文字。8,781 の回だけセッションが medium で、表にない 7 回目を含む他の回は max だった。回数でも伸びているので、effort の効果とは切り分けられない。
- **復元の上限(1 バージョンのバイナリから読んだ値):** ファイルは 1 件 5,000 token、合計 50,000 token。スキルは 1 件 5,000 token、合計 25,000 token。変更できる設定は見つからなかった。

### window の計算(本レポートと同じ単価)
床 95k、増分 3k / リクエスト、read $0.20、write $8、要約 output $20(/MTok)、要約 8k token、床の書き込み 65k で計算した。1 リクエストあたりの費用は read と compaction だけで、通常の output と増分の write は含まない。200,000 の 24 は実測の中央値(約 22)に近い。

| window | T | 余裕 | compaction あたりのリクエスト | 費用 / リクエスト |
|---|---:|---:|---:|---:|
| 200,000 | 167k | 72k | 24 | $0.056 |
| 300,000 | 267k | 172k | 57 | $0.049 |
| 400,000 | 367k | 272k | 91 | $0.054 |
| 1,000,000 | 967k | 872k | 291 | $0.109 |

頻度を下げることを優先して 400,000 にした。`/context` では `claude-opus-5-5`、`opus`、`opus[1m]` のいずれも分母が 400k、buffer が 33k だった。**400,000 での実際の発動は未観測である。** T=367k での品質は不明である(レポート 3 章)。

### `/effort` の確認ダイアログ(セクション 5 の検証の見落とし)
- `claude-opus-5-5[1m]` の直接接続で、ターン終了後に mod の `/effort max` が「Change effort level?」を出した。文面は「Switching to max means the full history gets re-read on your next message」。回答するまでキューは止まる。
- 同じ操作で `CLAUDE_CODE_TETHER_LIVE=0` だけ変えると出なかった。プロキシ経由(`claude-opus-5-5`)でも出なかった。各 1 回。条件はバイナリの `MWe` で、キャッシュが有効で、compaction 後に出力が増えていて、直近のリクエストが threaded のとき。compaction 直後の復元は確認済み扱いになるので出ない。
- `No, go back` は `Kept effort level as <level>` を返す。キューの `/compact` はそのまま元の level で実行される。mod の `session` は max と記録されるが、続く復元で正しい値に戻る。
- 実セッションでは、ターンを止める経路の引き上げ 2 回では出ず、ターン終了後の 2 回で待ちが起きた。経路で違う理由は未確認。
- 同じ設定の再現(プロキシ経由)で通信を記録すると、compaction の要求は `effort=max`(出力 4,520)で、その間の画面は `🧠 medium` だった。**threaded が有効な経路の compaction は通信を確認していない。**

### キューの順序(実セッション)
- `$.command.run` と `$.prompt.submit` は常に優先度 `later` で積まれ、引数で変えられない(バイナリ)。人のプロンプトは `next` で先に実行される。
- compaction 中のプロンプトが先に実行され、さらに `/goal` がターンを続けたため、`/effort` の復元は compaction 完了の 5 分 28 秒後になった。その間のメイン要求 28 件はすべて medium で送られた。spinner の `with max effort` はセッションの値の表示である。
- **復元が長く実行されないことがある:** 17:38 に max へ上げた後の `/effort medium` は、18:22 の時点でも `/goal` が続いていて実行されていない。この間の 5〜7 回目は `auto` で、復元待ちのため mod の順序を通らず、ターンを止めずにセッションの effort(max)で動いた。17:38 から 18:20 までのメイン要求 61 件はすべて medium だった。
- **未修正の不具合:** 続きのプロンプトも同じ時刻に届き、不要なターン(要求 3 件)が走った。

## 未確認
- 回答品質に effort がどう効くか。
- 1M window を維持して 167k などの高い閾値で同じ動作になるか(window 100k と 200k の `/context`、および 100k の発動のみ確認)。
- 約 54 分放置後の `auto` compaction がどの機能か。
- `context` 以外の方法(別の text block など)で tool loop 途中の変更を効かせられるか。
