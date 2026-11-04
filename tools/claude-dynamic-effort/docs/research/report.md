# `tools/claude-dynamic-effort` を Claude Opus 5.5 向けにどう見直すか

調査日: 2026-11-05。対象コードは `dotfiles/tools/claude-dynamic-effort/hooks/register.ts`(読むだけで編集していない)。行番号はこのファイルのもの。

## 結論

1. **tool loop の途中で effort を変えても、その step の出力量は変わらなかった。上げても下げても同じで、新しいユーザーターンでは変わった。** 現行の「step ごとの追従」は、この条件では機能していない。実質的に効く判断点は、ターンの最初の step だけである。
2. **ramp と recovery floor の実際の動作は、設計の説明と違う。** ramp は「ターン開始時の 1 回の判断」になっている。floor は「前のターンの未解決の失敗を、次のユーザーターンに持ち越す」動作になっている。stuck note の「最高 effort で失敗した」という文言は、多くの場合に事実と合わない。effort の表示は、効いていない level を示すことがある。
3. **現行の 870k は max の能力から決まった値ではない。** 窓 1M から機械的に出た値(967k × 0.9)である。
4. **「medium で扱い切れる token 数」を Opus 5.5 で測った事実は存在しない。** 事実が絞れるのは下側だけである。`autoCompactWindow` の下限は複数の測定から **約 200,000**(エンジン閾値で約 167k)と言える。100,000 は事実で除外できる。**上限は事実では閉じない。** 測定する区間として 200,000〜400,000 を提案するが、400,000 は 1 本の論文の比較範囲に合わせた目安であり、支持された値ではない。1 点を選ぶ最も安い測定は 4 章に示す。
5. **`autoCompactWindow` を下げるだけでは要求に反する。** ramp は閾値に対する比率なので、200,000 にすると約 100k で high、約 134k で xhigh に上がる。ramp を外すか、閾値から切り離す必要がある。
6. **compaction を max で行う利益を示す事実は見つからなかった。** この点は要求文の前提(max で compact する)から外れるが、session effort への変更を推奨する。別の方法の根拠は複数あり、max が届くのはターン終了時の一部の compaction だけでもある。ただし害も証明されていないので、品質の判定は未確定である。
7. 2〜6 をすべて適用すると、この mod の「動的 effort」部分はほぼなくなる。残るのは stuck note、compaction の guideline、表示の短縮である。段階を分けて判断できるよう、6 章に順序を示す。

## 証拠の区分

本文では次の区分を混ぜない。根拠のノートは同じディレクトリの `notes/` にある。

| 区分 | 内容 | 限界 |
|---|---|---|
| **直接測定(実験)** | `capture_experiments.md`。Claude Code 2.1.304、claude-opus-5-5、サブスクリプション認証、ローカル記録用プロキシ | 1 種類の課題、各条件 5 回、1 バージョン。出力 token は thinking を含む努力量の代理指標で、品質ではない |
| **直接測定(観察)** | `local_session_measurements.md`。1 利用者の `cli` 50 セッション(Opus 5.5 は 41)、約 33 日 | Opus は最多 1 セッションがリクエストの 36%、上位 5 セッションが read の 81%。一般化できない。870k の行はリプレイで実測ではない |
| **cache の実測** | `audit.md` 7 章。Opus 5.5、変更 269 組 / 14 session、対照 28,468 組 | 1 利用者、1 バージョン、約 1 週間 |
| **コード読解** | `register.ts`、plugin 型定義 `.claude-plugin/types/claude-code/index.d.ts`、Claude Code バイナリ(`audit.md`) | 実行時の値までは確認していない箇所がある |
| **ベンダー文書** | `opus_5_5_guidance.md`。Anthropic / OpenAI の docs、system card | 「Anthropic's testing」の値は公開データ付きの比較ではなく、独立に検証されていない |
| **文献** | 5 本のノートの論文。`audit.md` の修正値を適用した | 大半は査読前の preprint。Opus 5.5 を対象にした研究は 1 本もない |
| **計算 / 推論** | 本レポートで計算した値、および事実からの推論 | その都度明記する |

**本レポートは統計的メタ分析ではなく、叙述的統合(narrative synthesis)である。** 指標(accuracy、Mean Match Ratio、rule 喪失数、違反率)、課題、モデルが研究間で揃わず、多くの研究が分散を報告していないので、効果量を統合できない。研究間の比較は方向と条件の比較にとどめ、統合値は出していない。

確信度の表記は次のとおり。**高** = 独立した複数の証拠が同じ方向で、適用条件も近い。**中** = 複数の証拠があるが条件に距離がある。**低** = 間接証拠のみ、または証拠が割れている。**なし** = 判断できる証拠がない。

`audit.md` は他のノートより優先した。「(監査済)」は `audit.md` が原典と照合した値を指す。なお `audit.md` の「effort が効いているかは判定不能」は transcript からの判定で、その後に行われた `capture_experiments.md` の対照実験が、限られた条件でこの点に答えている。両者は矛盾しない。

---

## 1. 設計を変える発見: ターン途中の effort 変更は出力量を変えなかった

### 測定結果(直接測定、各条件 n=5)

課題は Bash で date を実行した後の手計算である。tool_result 直後の step の出力 token を測った。

| 条件 | 送信された top-level / per-message | 出力 token(昇順) | 中央値 |
|---|---|---|---|
| low → low | low / low | 2,936, 3,285, 4,506, 4,781, 5,994 | 4,506 |
| low → max(ターン途中) | max / low, max | 2,910, 3,170, 3,200, 5,241, 6,954 | 3,200 |
| low → max(ターン途中、tool result に `context` を追加) | max / low, max | 3,440, 3,883, 3,951, 5,019, 6,612 | 3,951 |
| max → max | max / max | 18,897, 19,889, 20,962, 21,428, 25,309 | 20,962 |
| max → low(ターン途中) | low / max, low | 16,210, 16,781, 19,986, 21,792, 22,107 | 19,986 |
| 新しいユーザーターンで low → low(tool なし) | | 2,918, 3,132, 3,395, 4,444, 4,781 | 3,395 |
| 新しいユーザーターンで low → max(tool なし) | | 15,142, 17,619, 19,389, 21,173, 35,452 | 19,389 |

読み取りの強さは次のとおり。

- **新しいターンの変更は明確に効いた。** 中央値で 5.7 倍、分布は完全に分離している(5 対 5 の正確検定で両側 p≈0.008、本レポートの計算)。
- **ターン途中の low → max は max → max の範囲に届かない。** max → low も low → low の範囲まで下がらない。どちらも完全に分離している。つまり、新しいターンで見られた大きさの効果は否定される。
- **「効果がまったくない」とまでは言えない。** low → low と low → max は範囲が重なるが、n=5 では小さな差を検出できない。
- **文書と一致する。** Effort ドキュメントの「tool_result だけの user ターンは新しい入力に数えない」と同じ結果である。文書の記述と今回の測定という、別系統の 2 つが一致している。
- **top-level の値はターン途中でも変わっていた。** 上の表のとおり max や low に変わっていたのに、出力量は変わらなかった。per-message が優先されていると見られるが、優先関係そのものは未確認である。top-level だけを細工して回避できるとは言えない。

### mod に当てはまる理由と、当てはまらない範囲

実験は mod ではなく実験用 plugin で行われた。mod に当てはめられるのは、mod が step で effort を変えたときも top-level と per-message の両方が同じ形で変わり、beta も `per-turn-control-2026-07-01` だと観測されているからである(`capture_experiments.md` 2 章、`audit.md` の 269 組)。

適用範囲の限界は次の 4 点である。

- 測ったのは **tool_result だけのターンの直後の step** である。ターン内の 3 step 目以降は測っていない(文書は同じ扱いだと述べる)。
- tool_result と text が同居する位置は未測定である。`audit.md` の wire 判定でも 279 件中 43 件しか位置が分からず、うち 10 件はこの形だった。
- 測ったのは low と max の両端だけで、medium、high、xhigh は未測定である。
- 品質は測っていない。effort が品質に効くかどうかは、この実験から何も言えない。

### 現行コードの各部品への意味

| 部品 | 設計上の意図 | 実験とコードから見た実際の動作 |
|---|---|---|
| **ramp**(`rampEffort` 279–285 行、`RAMP_START_RATIO` 169 行) | context が閾値の 60% を超えると上げる。既定の medium 開始なら 60〜80% で high、80% 超で xhigh(`rampStep` 0.2) | 効くのはターンの index 0 の step だけで、参照するのは前のターン末尾の文脈量である。**実質はターン単位の判断**で、長い tool loop の途中で閾値を越えても、そのターンの思考量は変わらない。「ramp が動いていない」のではない |
| **recovery floor**(`nextFloor` 295–307 行、`withRecovery` 322 行) | 失敗で上げ、verified fix で即座に下げ、clean 2 回で下げる。repeat と malformed は維持 | 失敗の次の step は定義上 tool_result だけのターンなので、**上げても効かない**。max → low が効かないので、**「verified fix で即座に下げる」も効かない**。`floor` はターンをまたいで残り、tool round が来るまで下がらない(compaction でも残る。838 行のコメント)。したがって**前のターンの終了時に残った値が、次のユーザーターンの index 0 に乗る**。どの課題が来ても同じで、ターン全体がその level になる |
| **stuck note**(`STUCK_NOTE` 159 行、`claimsStuckNote` 695 行、`noteToolResult` 734 行) | cap で失敗が続いたら、方針変更を促す 1 行を tool result に足す | 発火条件は `floor.steps >= last` で、失敗回数の数であり実際の level ではない。既定では **5 回連続の失敗 round で発火する**(`register.test.ts` 923–944 行で確認)。文面の「at the highest work effort」は、ターンが medium で始まっていれば事実と合わない。なお note は内容であり、effort の変更に依存しない仕組みである。`context` を足しても effort は効かなかったが、**モデルがこの note に従うかは別の問いで、未測定である** |
| **表示**(`sendEffort` 581 行、`dynamicTail` 588 行) | 送った level を表示する | tool loop の途中では、**効いていない level を表示する**。例えばターンが medium で始まり、失敗で xhigh と表示されていても、出力量は medium のままと考えられる。transcript の `assistant.effort` も同じく送信値である。`local_session_measurements.md` 7 章の effort 別の表は「効いた level」ではなく「送った level」として読む必要がある |

### 実害の大きさ

- **cache の費用はほぼない。** effort 変更群の hit 率中央値は 0.9949(269 組)、対照は 0.9965(28,468 組)で、実験でも cache read は継続した(直接測定)。step ごとの書き換えの問題は費用ではなく、効かないことと、表示と説明が事実と違うことである。測定は subscription 経由で、docs は API key でも保たれると述べる。Bedrock などでは docs によれば保たれない。
- **発生頻度は小さい。** 観察期間の Opus 5.5 のリクエストは medium が 94.7%、high が 0.9%、xhigh が 0.3% である(送信値)。mod の有効期間は確認していない。
- **transcript 上の effort 変更は大半が loop 内である。** user レコードで見ると 283 件中 273 件が tool_result のみの直後である。ただしこれは wire の形ではなく、参考にとどまる(監査済)。

### コメントと README の根拠

`register.ts` 131–135 行と README の「Decide per step」は ARES を根拠にしている。ARES は gpt-oss-20b で推論ごとに effort を変えられる環境の研究で、Opus 5.5 の tool loop ではこの操作自体が今回の条件では成立しなかった。**per-step 選択の文献は、この環境の tool loop には適用できない。** ターン単位の判断に読み替えられるかを検証した研究も見つからなかった。

---

## 2. 五つの問いの横断比較

表の「監査」欄は `audit.md` が原典と照合した数値かどうかを示す。

### 問い 1: context の劣化(どこから、何に依存して性能が落ちるか)

| 研究 | モデル / 課題 | 結果 | 監査 | 主な制約 |
|---|---|---|---|---|
| [GPT-5.4](https://openai.com/index/introducing-gpt-5-4/) | MRCR v2、xhigh | 帯順に 97.3、91.4、97.2、90.5、86.0、79.3、57.5、36.6 | 済 | r.jina.ai 経由。合成の検索課題 |
| [GPT-5.5](https://openai.com/index/introducing-gpt-5-5/) | MRCR v2、xhigh | 98.1、93.0、96.5、90.0、83.1、87.5、81.5、74.0 | 済 | 第二の出典なし。非単調 |
| [Opus 4.6 card](https://www.anthropic.com/claude-opus-4-6-system-card) | MRCR v2 | 256K 帯 91.9 → 1M 帯 78.3(64k thinking)、max は 93.0 → 76.0 | 未 | ベンダー。5 trial |
| [Sonnet 4.6 card](https://www.anthropic.com/claude-sonnet-4-6-system-card) | MRCR v2 | max 90.3 → 65.8。API 上限内の subset は 77.8(29 問) | 済 | 65.8 は API の 1M 上限を超える問題を含み、他社値と同一条件ではない |
| [Opus 4.7 card](https://www.anthropic.com/claude-opus-4-7-system-card) | MRCR v2、max | 59.2 → 32.2 | 一部 | 256K 帯ですでに低い |
| [LongBioBench](https://arxiv.org/html/2506.02921v2) | Qwen2.5-7B、128k standard | 131K 窓版 26.1、1M 版 94.9。multihop_2 は 1.5 / 25.9 | 済 | 別 checkpoint。1.5 は監査の修正値 |
| [LongCodeBench](https://arxiv.org/html/2505.07897) | Claude 3.5 Sonnet | SWE は 32K→256K で 29→3%、QA はほぼ平坦 | 未 | 旧世代。帯ごとに問題が違う |
| [Xue](https://arxiv.org/html/2607.17937v2) | Codex + gpt-5.4-mini、code audit | clean 8/10、299,140 字 3/10、Fisher p=0.0698 | 済 | 1 課題、探索的、p は 0.05 超 |
| [Fan ほか](https://arxiv.org/html/2609.20804) | open-weight、SWE-bench | 管理あり − なしの差は 32k で 35.7pp、128k で 2.7pp | 未 | 人工予算で 1M 級は扱わない |
| [Chen ほか](https://arxiv.org/html/2603.22608) | 16 モデル、multi-instance | instance 数を固定すると context 長の相関は有意でなくなる | 未 | agent 課題ではない |

- **一致点:** 普遍的な閾値はない。劣化はモデル、課題、内容で変わる。2026 世代の検索系課題では 256K より上で低下が明瞭になりやすい。
- **矛盾:** ベンダーの「1M まで精度を維持」と自社 card の低下。Chroma の「長さで一貫して劣化」と Chen の「長さ単独では説明力が小さい」。Opus 4.7 は世代が新しいのに 4.6 より低い。位置効果は研究で分かれる。
- **適用できない条件:** MRCR と GraphWalks は 1 回の長い prompt で測る合成課題で、tool を重ねる agent とは別物である。旧世代の絶対値は流用できない。**Opus 5.5 の context 長別データは読んだ範囲に存在しない。**
- **確信度:** 「単一の閾値は存在しない」は**高**。「窓比でも絶対値でも単純には説明できない」は**中**。「Opus 5.5 の medium が扱える token 数」は**なし**。

### 問い 2: compaction の trigger

| 研究 | 操作 | モデル / 課題 | 結果 | 監査 | 主な制約 |
|---|---|---|---|---|---|
| [2610.31778](https://arxiv.org/html/2610.31778) | 固定 160K〜380K | Opus 4.6、GPT-5.4(1M)、GPT-5.3-Codex(400K) | 1M の 2 モデルは差を検出せず。Codex は PaperBench で 160K 0.38 → 260K 以上で約 0.60 | 済 | 1 閾値あたり少数 run なので「差なし」ではなく「検出されない」。Codex の 160K は 27.71 ± 17.00 回 compaction。scaffold 既定付近は未検証 |
| [Bud ContextCompact 2.0](https://arxiv.org/abs/2610.25516) | 22.4K vs off、24K/32K/48K | Qwen3.8-27B、SWE-bench | 長いセッションで 41→53%(p=0.043)。45 分予算では −3.3pp(p=0.48) | 未 | 小さい窓 |
| [Beyond Token Savings](https://arxiv.org/abs/2609.32961) | 10K/15K/20K | Qwen3.5-35B など | Qwen は 50.7 → 73.3%、Devstral は 72〜73% で不変 | 未 | 小型モデル。「遅いほど良い」を 1M に外挿できない |
| [CliffCompaction](https://arxiv.org/abs/2609.26779) | 45K〜8K、要約なし | Kimi K2.6、GLM 5.1 | full と 32K / 16K の差は小さく、8K で低下 | 未 | rule-based |
| [SelfCompact](https://arxiv.org/abs/2606.23525) | モデル判断 vs 窓 30% vs なし | GLM-4.7-Flash など、探索と数学 | BrowseComp-Plus の対固定は +4.1 / +5.3 / +2.7(+6.3 は BrowseComp の Mimo)。Overall +4.9 / +4.6 / +4.5 | 済 | coding を含まない。**費用は固定閾値より 12 セル中 8 セルで高い**。BrowseComp-Plus の Mimo は Keep-last-N が上回る |
| [Slipstream](https://arxiv.org/abs/2605.08580) | 同閾値で同期 vs 非同期 | Qwen3.5-9B など | 閾値に単調傾向なしと著者が明記 | 未 | 非同期が主題 |

ローカルの測定は別に示す(直接測定)。リプレイは実測の文脈増分から再構成したもので、Opus 5.5(n=41)の値である。

| ターン終了時の発動点 | 圧縮回数 /100 ユーザーターン | 増えた圧縮 1 回あたりの read 削減 | read+圧縮 read(870k 比) |
|---|---:|---:|---:|
| 870k | 8.7 | - | 100% |
| 580k | 9.6 | 126,856,930 | 85% |
| 400k | 11.1 | 88,596,661 | 73% |
| 300k | 13.6 | 58,864,308 | 63% |
| 200k | 19.2 | 35,094,866 | 53% |
| 150k | 27.8 | 22,051,311 | 46% |
| 100k | 53.7 | 10,343,393 | 40% |

他に重要な測定値は次のとおり。

- **圧縮直後の文脈(床):** Opus で p50 71,578、p90 79,997、max 89,482(n=95)。圧縮前の大きさと相関しない(0.07)。ただし**圧縮前 200k 以上でしか観測していない。**
- **ターン増分(Opus):** ユーザーターン p50 10,531、p90 82,737。1 リクエストの最大は 48,326。
- **越境の位置:** 閾値を越える位置の 88〜93%(Opus エンジンターン、ユーザーターンでは 96〜99%)はターンの先頭か途中である。ターン終了時の判定だけでは止められない。
- **`autoCompactWindow` の実機動作:** window 100,000 で、エンジンはターン途中に約 65〜66k で compaction した(直接測定)。閾値の式は `窓 − min(maxOutputTokens, 20000) − 13000` で、`/context` の 200k→167k、1M→967k と一致する(`audit.md`、コード読解)。13K と 33K の不一致は基準点の違いで解消済みである。

**ベンダー文書との区別:** `compaction` パラメータも `compact-2026-09-04` beta も `/compact` では送られていなかった(直接測定)。API の threshold compaction の既定 150,000 や最小 50,000 は、**Claude Code の trigger の根拠にならない。**

- **一致点:** 頻度を上げすぎると費用面の利得が逓減する(ローカルのリプレイ、2609.32961、2606.11213)。閾値の感度はモデル依存が大きい(2610.31778、2609.32961)。
- **矛盾:** 2610.31778 の 1M モデルは 160K でも差が出ないが、400K 窓の Codex は 160K で落ちる。SelfCompact は適応が優位とするが、費用は固定閾値の方が多くのセルで安い。
- **適用できない条件:** 380K を超える比較研究はない。ターン終了時とターン途中の直接比較、task boundary と閾値の coding での比較も見つからなかった。Opus 5.5 を含む sweep はない。vendor の既定値(窓の 50%〜上限)はどれも実務であり、根拠ではない。
- **確信度:** 「870k の妥当性」は**なし**(支持も反証もない)。「1M の frontier モデルで 160K〜380K に大きな差はない」は**低〜中**(少数 run、Opus 5.5 ではない)。「window 100,000 は運用できない」は**高**(直接測定と観察が一致。4 章)。

### 問い 3: effort と context の相互作用

| 研究 | 操作 | 結果 | 監査 | 主な制約 |
|---|---|---|---|---|
| **直接測定**(`capture_experiments.md`) | ターン途中 vs 新規ターンの effort 変更 | 1 章のとおり | - | n=5、low と max のみ、品質は未測定 |
| [OOLONG](https://arxiv.org/abs/2511.02817) | GPT-5-nano high/low × 8K〜256K | 64k 以降で差がほぼなく、256k で high がわずかに下 | 済 | 図のみ。1 モデル |
| [CL-bench](https://arxiv.org/abs/2602.03587) | 9 モデル、長さ bin 別(観察的) | 平均差 +1.51 → +1.39(全 bin でも拡大傾向なし)。拡大 4 / 縮小 4 / 不変 1。相対比は 5 モデルで拡大 | 済 | 著者は「長いほど顕著」と書くが検定なし。API effort 3 モデルの +0.5 → +2.0 は GPT 5.1 依存で、除くと +0.3 → −0.75 |
| [Reasoning Shift](https://arxiv.org/abs/2604.01161) | prompt の max effort × 無関係な長文 | Qwen 67.8 → 70.5、Gemma 67.0 → 70.9、gpt-oss 64.0 → 61.8。除去の方が効く | 済 | Claude なし。合成条件。tokens の 10〜15% 増は Gemma と gpt-oss のみで Qwen は増えない。74% 減は Long Input ではなく Multi-turn |
| [ARES](https://arxiv.org/abs/2603.07915) | per-step router | TAU Airline で medium 42.0 / 98k、high 38.0 / 873k、RL 42.0 / 133k | 済 | gpt-oss-20b。**42.0 vs 38.0 は約 50 課題中 21 対 19 で 2 課題差**(CI の記載なし) |
| [PredicateLongBench](https://arxiv.org/abs/2607.08284) | effort × 探索難度(長さ固定) | 「Budget alone does not close the decoy gap」 | 未 | 長さとの交差ではない |
| Anthropic docs | [Effort](https://platform.claude.com/docs/en/build-with-claude/effort)、[Prompting Opus 5.5](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5) | 「Reserve `xhigh` and `max` for work where you've measured a quality gain」「Start at `medium`」。動的 effort の基準は task complexity で context 量ではない | 一部 | ベンダー、独立検証なし |

- **一致点:** context 量を trigger に effort を上げる方針を評価した研究は 0 件である。除去や compaction が effort 増より有利という方向は、Reasoning Shift と Anthropic の指針が同じ方向を向く。ただし Reasoning Shift の無関係文は除去に損失がないので、compaction の利得の上限として読む。
- **矛盾:** OOLONG(長いと利得が消える)と CL-bench 著者の記述(長いと顕著)。CL-bench の数値を再計算すると、絶対差では拡大しない。
- **適用できない条件:** Opus 5.5 では `budget_tokens` が 400 エラーなので、固定予算の研究(TALE、Snell)の条件は再現できない。level 名はモデル間で等価ではない(ベンダー文書)。per-step の研究はこの環境の tool loop に適用できない(1 章)。
- **確信度:** 「ターン途中の変更はその step の出力量を大きくは変えない」は**中**(対照実験で文書とも一致するが、1 種類の課題)。「context に応じて上げる方針を支持する証拠はない」は**中〜高**(不在の主張だが、複数系統で探している)。「上げると害がある」は**低**。

### 問い 4: 要約時の effort

| 研究 | 操作 | 結果 | 監査 | 主な制約 |
|---|---|---|---|---|
| **直接測定**(`capture_experiments.md`) | `/compact` の effort | session の値がそのまま送られる。極小の会話で low 345 / max 2,024。mod の実機で medium 945〜1,133 / max 2,658(約 66〜69k) | - | 各 1 回〜数回。品質は未測定。thinking と可視の内訳は未分離 |
| README の測定 | 同上 | low 934 / max 1,898 | - | 1 会話 |
| [2512.03503](https://arxiv.org/html/2512.03503v1) Fig. 6 | GPT-5 の think ability、文書要約 | **全データセットで単調に下がるわけではない**。ArXiv は 0.915 / 0.952 / 0.77 / 0.635、WikiHow は 0.985 / 0.964 / 0.85 / 0.735、SciGen は下がらない | 済 | 図のみ。Fig. 6 と Table 5 の値が食い違う。compaction でも agent でもない |
| [2610.31778](https://arxiv.org/html/2610.31778) | rule 喪失(Table 3) | Opus 4.6(max)0.51、GPT-5.4(medium)1.76、GPT-5.3-Codex(medium)2.73 | 済 | **effort とモデルが完全に交絡。** ProgramBench 3 課題。effort の効果とは読めない |
| [marina PR #529](https://github.com/h2oai/marina/pull/529) | reasoning off | fact retention +0.27〜+0.42 | 未 | 原因は 500 token 上限の枯渇。Claude Code の `max_tokens` は 128,000(直接測定)なので**この機序は当てはまらない**。査読なし |
| [Bud ContextCompact 2.0](https://arxiv.org/html/2610.25516v1) | compaction 有無(reasoning 継承) | 1 回 161 秒の中央値、45 分予算で利得消失 | 未 | effort を変えていない |
| [ACON](https://arxiv.org/html/2510.00615) | guideline 最適化 | 精度 43.5 → 51.2 / 56.5(No compression 56.0、168 課題)。**peak は Prompting 6.93K から 7.17K / 7.33K に増える** | 済 | 効果は精度の向上で peak 削減ではない |
| [Governance Decay](https://arxiv.org/html/2606.22528v2) | pinning、summarizer、budget | pinning 約 47 tokens で違反 0%(9 課題、27/cell、7 モデル)。保持 0%(n=90)、脱落 38%(n=315)。budget 300→15 語で生存 88→23% | 済 | operator 詐称で pinning 0→17% |
| [Lost in Context Compaction](https://arxiv.org/html/2608.11242v2) | extractor、prompt | 分離抽出で保持 平均 93.0%。targeted prompt は 1.3→24.8、3.3→37.6。Pi prompt は 88.4、98.0、6.7 とデータセットで逆転 | 済 | モデルは gpt-oss、Qwen、GPT-5.4-mini。Claude の effort は操作していない |

- **一致点:** **要約者の effort だけを変えて downstream を測った研究は見つからなかった。** 喪失の中心は constraint の脱落で、構造的な分離(pinning、extractor)と summarizer の違いの効果は複数研究で大きい。ローカル測定では effort を上げると出力は 2 倍以上になる。
- **矛盾:** compaction の損失を「モデル非依存」とする 2603.17781 と、強いモデル依存を示す 2606.22528、2610.31778 は未解決である。guideline の効果は ACON で正だが、Lost in Context Compaction では prompt の効果がデータセットで逆転し、40% 未満にとどまる。2610.31778 では constraint の保持を求める prompt でも rule が落ちた。
- **適用できない条件:** 文書要約の faithfulness を compaction の品質に転用できるかは不明である。marina の機序は適用できない。API の Compaction on demand の記述(要約入力、custom instructions の扱い)は、`/compact` がこの API を使っていない以上、Claude Code には当てはまらない(直接測定)。
- **確信度:** 「max の利益を示す直接証拠がない」は**高**(不在の主張。検索は数系統)。「max は下流の品質を下げる」「上げない」は**なし**。「構造的な分離が guideline や effort より強いレバーである」は**中**(研究内比較のみで、effort と同時に測った研究はない)。

### 問い 5: 失敗時の escalation

| 研究 | 操作 | 結果 | 監査 | 主な制約 |
|---|---|---|---|---|
| [AI Agents That Matter](https://arxiv.org/abs/2407.01502) | failing test で model を上げる | Escalation 85.0% / $0.27、GPT-4 単独 89.6% / $1.93、Retry 92.0% / $2.51、Reflexion 87.8% / $3.90 | 済 | model の切替。旧世代。164 問 × 5 |
| [Handoff Tax](https://arxiv.org/abs/2608.24358) | 固定点で model を上げる | Claude pair: Raw 69.2% / $1.61、HC 79.2% / $0.72、LC 60.7% / $0.40 | 済 | **全 500 課題の平均ではなく switched subset の値。** 失敗起点ではない。model の切替 |
| [Mehta](https://arxiv.org/abs/2607.02436) | Opus 4.7 High vs xHigh を開始時に固定 | 5/18 → 16/18、差 61pt(CI 29–78)、abridged では 1/6 対 1/6 | 済 | **非ランダム化。反応的な引き上げの評価ではない。** Opus 4.7 で level 名は 5.5 と等価でない |
| [ToolBench-X](https://arxiv.org/abs/2606.25819) | 失敗後の追加 round と hint | TTS +3.5–11.5、Hint +25.5–35.5 | 済 | 「失敗した」ことを実験側が与える。**Hint は注入した異常の内容を教える**ので、汎用の方針変更 note の根拠にならない |
| [Self-Debug](https://arxiv.org/abs/2304.05128) | 実行結果 feedback で再生成 | GPT-4: TransCoder 77.3→90.4、MBPP 72.8→80.6、Spider 73.2→73.6 | 未 | effort 不変 |
| [Huang](https://arxiv.org/abs/2310.01798) | 外部 feedback なしの self-correct | GPT-4 GSM8K 95.5→91.5→89.0 | 未 | 旧世代 |
| [SWE-smith](https://arxiv.org/abs/2504.21798) | repeat の警告と resample | 40.2%→38.4%(192 件)。repeat は減った | 済 | 警告で resolve は上がらない例。主に SWE-agent-LM-32B |
| [ARES](https://arxiv.org/abs/2603.07915) | 学習 router | Airline の 2 課題差。go_back、branch に high が多い | 済 | 明示的な失敗 trigger はない |
| [Reasoning Trap](https://arxiv.org/abs/2510.22977) | thinking on/off | tool hallucination が増える例 | 未 | 小型モデル、単一 step。frontier の effort 階層ではない |

retry の上限は実験では 3〜10 回前後に集まるが、**上限値そのものを比較した研究は見つからなかった。**

- **一致点:** 失敗後の改善は、計算量より外部シグナルの情報量で説明しやすい(ToolBench-X、Self-Debug、Huang の oracle 条件)。Escalation は安いが最強モデル単独の精度には届かない例が多い。
- **矛盾:** Mehta は effort 増を支持するが開始時の固定である。Reflexion の 91% は AI Agents That Matter の再測定では 87.8% だった。Recovery Is a Decision(abstract のみ)では live 評価で外部 policy が native 処理に劣る。
- **適用できない条件:** **同一モデルの effort を失敗 trigger で変える policy を直接評価した研究は 0 件である。** model cascade の結果は effort の階層に直接移らない。verified recovery を trigger に下げる policy の評価も見つからなかった。何より、この環境の tool loop では失敗直後の step に effort が効かない(1 章)。
- **確信度:** 「失敗直後の step を effort で救えない」は**中**(直接測定)。「次のターンへの持ち越しに利益がある」は**なし**。「情報のある外部シグナルは計算量より強い」は**中**。「汎用の stuck note が役立つ」は**低**(SWE-smith では警告で resolve は上がらなかったが、文面も条件も同じではない)。

---

## 3. 実装案の比較

### 3.1 compaction の trigger

| 案 | 事実として確立していること | 推論にとどまること |
|---|---|---|
| **A. 絶対値(`autoCompactWindow`)** | 設定できる範囲は 100,000〜1,000,000(docs)。環境変数 `CLAUDE_CODE_AUTO_COMPACT_WINDOW=100000` で、エンジンはターン途中でも設定した window に従って発動した(直接測定)。mod の順序も同じ条件で動いた。閾値は window − 33k(1 バージョンのコード読解)。`/context` は 100k と 200k で確認済み | **settings の `autoCompactWindow` が発動でも環境変数と同じ動作をするか**(`--settings` で 200,000 を渡すと `/context` は 200k / buffer 33k を示したが、発動は観測していない。`index.d.ts` 527 行の同名の欄は subagent 定義のもの)。1M window を保ったまま高い閾値で同じ動作をするか(200k〜400k の発動は未確認)。Sonnet など他モデルや subagent への波及。「medium で扱える」を満たすか |
| **B. 窓比率(`CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`、または mod の比率)** | 下げる方向のみ(docs)。比率を独立変数にした研究は見つからなかった。2610.31778 は閾値をモデル間で比較できないと明記する。同じ帯でも 512K-1M は GPT-5.4 36.6、GPT-5.5 74.0(同じ xhigh) | モデルを替えると token 量が変わるので、要求の「token 数で決める」とは別物になる。単一モデル、単一窓なら A と数値上は同じになる |
| **C. 適応(モデル判断)** | 直接比較は SelfCompact の 1 本だけで、coding を含まず、費用は固定閾値より 12 セル中 8 セルで高い。Claude Code に同じ機構はない | coding への移転。**単一論文なので採用できない** |
| **C'. 適応(増加速度で閾値を変える)** | 計算上の最適閾値は増分 g で動く(compaction_trigger.md の EOQ 式)。g と S の分布はローカル測定でわかった | 比較実験はない。品質を含まない |
| **D. mod だけで絶対値を持つ(ターン終了時のみ)** | エンジンの設定を変えない。越境の 88〜93%(Opus)はターンの先頭か途中で、ピークは閾値の最大 3.95 倍(200k)〜7.89 倍(100k)に達する(リプレイ。max は同じ 1 ターン由来)。エンジンターン越境の超過 p90 は約 85k〜200k | 「上限」として機能しない。mod が設定を書けるかは未確認 |

**判断:** A を採用する。要求(token 数で決める)と形が一致し、ターン途中の上限まで実機で効くことが確認された唯一の案である。C は単一論文で採用できない。B は要求と形が違う。D は上限にならない。

### 3.2 effort の決め方

| 案 | 事実として確立していること | 推論にとどまること |
|---|---|---|
| **E1. ターンの最初の step で決めて固定** | 新規ターンでの変更は効く(n=5、5.7 倍)。ターン内で値が一定になれば、表示が事実と一致する。文書の「Hold top-level effort constant」とも一致する | `index === 0` が常に新しい入力の直後か(task-notification、キュー入力、compaction 後は未確認)。何を入力に決めるか |
| **E2. step ごとの書き換えを維持** | cache は保たれる(269 組)。tool loop の途中では出力量を変えない(n=5) | 将来の版で挙動が変わる可能性。**tool_result + text の位置で効くか**(未測定) |
| **E3. ramp を外す(session effort のまま)** | context 量で上げる方針を評価した研究は 0 件。OOLONG は逆方向で、CL-bench の再計算でも拡大しない。Anthropic の指針は medium から始め、xhigh と max は測定した場合に限る。ローカルに測定はない | 品質が落ちないこと。**これは測られていない。** ramp を保つ根拠もまた測られていないので、どちらも推論である。違いは費用で、新規ターンの low → max は出力を約 5.7 倍にする(medium → high の倍率は不明) |

**判断:** E1 は事実で支持できるので先に行う。その上で E3(context ramp の除去)を推奨する。理由は 3 つある。context 量を理由に上げる根拠がないこと、要求どおり「medium が扱える量で compact する」なら上げる論理的な理由が消えること、そして閾値を下げると比率の ramp が次の表のとおり要求に反して動くことである。

| `autoCompactWindow` | エンジン閾値 T | ターン終了時の発動 P(0.9T) | 現行 ramp の high / xhigh 開始 | 床 p50 / T |
|---|---:|---:|---|---:|
| 100,000 | 67k | 60.3k | 40.2k / 53.6k | 1.07(**床の時点で常に xhigh**) |
| 150,000 | 117k | 105.3k | 70.2k / 93.6k | 0.61(床の時点で high) |
| 200,000 | 167k | 150.3k | 100.2k / 133.6k | 0.43 |
| 300,000 | 267k | 240.3k | 160.2k / 213.6k | 0.27 |
| 400,000 | 367k | 330.3k | 220.2k / 293.6k | 0.20 |
| 1,000,000(現行) | 967k | 870.3k | 580.2k / 773.6k | 0.07 |

T は window − 33k、P は `PREEMPT_RATIO` 0.9 による(いずれも計算)。

failure 起点の引き上げ(floor)も同様に外すことを推奨する。根拠は、loop 内では効かないこと(直接測定)、失敗後は情報の方が効くこと(ToolBench-X、Self-Debug)、持ち越しの動作を評価した研究がないことである。Mehta は開始時の高 effort を支持するが、これは利用者が `/effort` で選ぶ使い方を支持するもので、自動の持ち越しを支持しない。**`/effort` と `minEffort` の利用者の選択はそのまま残す。**

### 3.3 compaction の effort

| 案 | 事実として確立していること | 推論にとどまること |
|---|---|---|
| **F1. max を維持** | 実機で動く(ターン終了時、2,658 出力 tokens)。ただし届くのは mod の順序を通る compaction だけで、エンジンのターン途中の compaction は medium で走った(直接測定)。`auto` の経路では effort を上げるコードがない(898–900 行)。**実データの圧縮の 81% は放置型で `auto` と記録される。** 順序には `/effort` の書き戻しの問題があり、`restoreSavesNothing` などの防御が必要である(コード読解)。下流の品質向上を示す研究はない | 品質が上がる可能性は否定されていない。Opus 4.6 の低い喪失率は effort かモデルか区別できない |
| **F2. session effort を使う** | Claude Code の素の動作である(直接測定)。出力 token は max の半分以下(945〜1,133 対 2,658。文脈量は近いが、前者はターン途中、後者はターン終了時で、回数も少ない)。`max_tokens` は 128,000 で出力枠の枯渇は問題にならない(直接測定) | 品質が同等かは未測定。増分が thinking なら 1 回限りだが、可視の要約なら後続の毎 request に乗る。内訳は未分離 |

**判断:** F2 を推奨する。根拠は、max の利益を示す研究がないこと(問い 4)、Anthropic の「測定した場合に限る」という指針、自前の測定で費用が 2 倍以上であること、max が実際の compaction の一部にしか届かないことである。**これは要求文の枠組みからの逸脱なので、利用者の判断事項とする。** max を残す根拠になるのは、4 章の測定 M5 で max の方が constraint の残存が明らかに高いという結果である。

guideline(`COMPACTION_GUIDELINE`)は維持してよい。ACON と Lost in Context Compaction は guideline に効果があることを示すが、後者はデータセットで逆転し、40% 未満にとどまる。pinning や分離抽出は別の研究で効果が大きいが、実装範囲が広がるので今回の提案には入れない(「他の選択肢」として記す)。

---

## 4. 推奨

### 事実で決められること

1. **ターン途中の effort の書き換えをやめる。** `index === 0` で決めた level をターン内で保持する。表示は自動的に効いている level と一致する。測定した範囲では効いていないものを止めるだけなので、品質への影響は見込まれない。ただし index 0 の例外条件は未確認である(7 章)。
2. **`autoCompactWindow` の 100,000 は使わない。** T=67k、P=60.3k である。Opus のセッション開始時の文脈は p50 52,525、p90 61,619 なので、作業を始める前に余地が約 5〜15k しかない。この事実は床の観測条件に依存しない。床の p50(71.6k、圧縮前 200k 以上で観測)も T を上回り、実機でも 1 回の実行で 2〜3 回発生した(人工条件)。ramp を残すと常に xhigh になる。リプレイの最低の行(100k、53.7 回 /100 ターン)でも P はこれより高いので、この設定はリプレイの範囲の外にある。
3. **`autoCompactWindow` を下げるなら、比率の ramp を同時に外すか切り離す。**
4. **stuck note の文面を直す。** 「at the highest work effort」は事実でなくなる。発火を失敗 round の数に切り離す。回数は事実で決まらないので(比較研究なし)、現行の既定(5 回目)を変える根拠はない。

### 範囲までしか絞れないこと: `autoCompactWindow`

**事実が絞れるのは下側だけで、1 点も上限も事実では決まらない。測定する区間として 200,000〜400,000(T は約 167k〜367k)を提案する。**

| 範囲の端 | 根拠 | 根拠の限界 |
|---|---|---|
| **下限 約 200,000(複数の事実で絞れる)** | ローカル測定が 3 つ揃う。P−床 p50 は約 79k で、Opus のユーザーターン増分 p90(82.7k)にほぼ等しい。費用は 150k 行(window 200,000 に相当)で底になる(下表)。これより下は圧縮回数が急増する(27.8 → 53.7 回 /100 ターン)。文献側でも、T=167k は 2610.31778 の最低水準 160K に近く、これより下で 1M の frontier モデルを比較した研究は見つからなかった。`/context` で値を確認済み | 床の測定は圧縮前 200k 以上のみ。費用はリプレイの計算。Codex は 400K 窓で 160K で低下しており、1M モデルの結果が Opus 5.5 に移る保証はない |
| **上限(事実では閉じない)** | 400,000 は T=367k で、比較研究の最上端 380K(2610.31778 の 1 本のみ)の内側に収まる。**これは測定する区間の目安であり、推奨の根拠ではない。** その 1 本も 1M モデルで差を「検出しなかった」だけである | 400k 超が悪いという事実はなく、良いという事実もない |

MRCR は区間の上半分に対する**弱い**反証である。T=367k は 256K〜512K 帯に入り、この帯の GPT-5.4 は 57.5(128K〜256K 帯は 79.3)、GPT-5.5 は 81.5(同 87.5)である。Opus 4.6 も 256K 帯 91.9 から 1M 帯 78.3 に下がる。ただし MRCR は agent の課題ではなく、Opus 5.5 のデータでもないので、区間を閉じる根拠にはしない。

範囲内の方向を決める力は両側にある。

- **下げる方向:** read の減少(リプレイ)、長文脈の劣化への備え(問い 1、MRCR の弱い証拠)、放置型 compaction の減少(5 章)。
- **上げる方向:** compaction 回数の増加と、それによる constraint 喪失の累積。0.92 → 1.93 → 2.41 → 2.65 の rule 喪失(2610.31778、全モデルプール、30 回目は信頼性が低い範囲)と、1/2/3 回で 91/62/46% の保持(2603.17781)はともに回数で悪化する。ただしリプレイは再読と手戻りを含まないので、低閾値に有利に出ている。

API 単価を掛けたリプレイは次のとおりである(**本レポートの計算**)。`local_session_measurements.md` は単価未確認として金額にしなかったが、`audit.md` が Opus 5.5 の単価を確認したので適用した。read と圧縮 read は $0.20、write は $5 / $8(5 分 / 1 時間)、要約 output は $20(いずれも /MTok)。通常の output と advisor は除く。Opus 5.5 の行である。

| ターン終了時の発動 P | 最も近い window | 圧縮回数 /100 ターン | 費用比(870k=1、write $5) | 費用比(write $8) |
|---|---|---:|---:|---:|
| 870k | 1,000,000 | 8.7 | 1.00 | 1.00 |
| 400k | 約 480,000 | 11.1 | 0.78 | 0.80 |
| 300k | 約 370,000 | 13.6 | 0.71 | 0.74 |
| 200k | 約 255,000 | 19.2 | 0.64 | 0.68 |
| 150k | **200,000** | 27.8 | 0.61 | 0.65 |
| 100k | 約 145,000 | 53.7 | 0.63 | 0.69 |

読み方の注意は 5 点ある。

- 費用は 150k 行で底になり、100k では低閾値に有利な仮定のもとでも**悪化する**。これが下限の根拠の 1 つである。全セッションの計算でも同じ形である(0.52 → 0.54)。
- このリプレイは**ターン終了時の発動だけ**をモデル化している。window を下げた場合に起きるターン途中の発動(越境の大半)は入っていない。実際の window 案の値はこの表と異なる。
- 放置型が続く前提である。再読、手戻り、品質低下は入っていない。
- 利用者はサブスクリプションなので、実際の制約は金額ではなく使用量の上限である。金額は比較の目安である。
- 計測期間は 33 日、上位 5 セッションが read の 81% を占める。

### 1 点を選ぶ測定

**何が 1 点を選ぶか:** Opus 5.5 の medium で、T の候補ごとの文脈量で課題の品質に差が出るかどうかである。差が出ないなら品質では決まらず、compaction 回数と費用で決める。

**最も安い測定: M4。** 既存の記録用プロキシを使い、次の順に行う。

| 順 | 測定 | 内容 | 費用の目安 | 答える問い |
|---|---|---|---:|---|
| 1 | **M1 実機確認** | window 200,000 と 400,000 で、T 付近の発動、`/context`、effort、1M window の扱いを各 1 回確認する | 小 | そもそも範囲内で同じ動作をするか |
| 2 | **M2 usage の記録** | advisor を使う main ターンで、`result.usage` と `session.usage().context.tokens` を並べて 1 回記録する | ほぼゼロ | 5 章の usage 問題 |
| 3 | **M3 リプレイ拡張** | `measure.py` にターン途中の発動 T を足し、window 200k、300k、400k で回数を再計算する | API 費用なし | 回数と費用の側の値 |
| 4 | **M4 品質プローブ** | 実 transcript の接頭部を 170k、270k、370k に切り、検証できる constraint と課題を埋め、**新しいユーザーターン**で medium に聞く。各サイズ 5 回程度 | 上限の見積もりで約 $20 + 出力(170k+270k+370k を 5 回ずつ、全量を $5/MTok の cache write とした計算。実際は 2 回目以降の多くが cache read になる)。サブスクリプションでは金額ではなく使用量の上限が制約である | 文脈量で品質が変わるか |
| 5 | **M5 要約 effort** | 同じ transcript を medium と max で `/compact` し、constraint の残存を比べる。thinking と可視の tokens を分ける。各 5 回程度 | 小 | max を残す根拠があるか |

**検出力の限界を先に言っておく。** n=10 の比較でも 8/10 対 3/10 は Fisher p=0.0698 で 5% に届かない(Xue の値)。n=5〜10 で検出できるのは大きな差だけである。したがって判定規則を事前に決める。

- 大きな差が出たら、差が出なかった最大の T を選ぶ。
- 差が検出されなければ、品質では決まらないと記録し、M3 の回数と費用で選ぶ。「差がない」とは書かない。

**測定前に設定する場合:** 事実は 1 点を選ばない。区間の下側(200,000)は `/context` が確認済みで、費用の表でも底にあり、MRCR の弱い証拠とも同じ方向である。上側は compaction 回数が少なく、回数で累積する constraint 喪失に有利で、放置型の扱いも現状に近い。これは利用者の判断事項である。

---

## 5. 54 分の放置後 compaction と usage の合算

### 約 54 分放置後の compaction

**確立していること(直接測定):**

- 全 compaction の多数を占める。全セッション 95/122(78%)、Opus 83/102(81%)。
- 直前は end_turn、無操作 53.67〜53.99 分に集中し、preTokens の最小は約 200k、trigger は `auto` と記録される。
- 予測力がある。end_turn 後に文脈 200k 以上かつ 53 分以上の間隔があった 129 回のうち 91 回(71%、Opus は 114 回中 80 回で 70%)で発生した。200k 未満の同条件では 112 回中 1 回(Opus は 88 回)で、その 1 回は直前の文脈がわずかに 200k 未満だった。
- 文書上の対応候補がある。settings-reference には「200,000 tokens 以上かつ約 1 時間無操作」の idle compaction の記述がある(`compaction_trigger.md` の引用)。条件はよく一致する。

なお `compaction_trigger.md` は idle compaction の対象を 1 時間 TTL の session と記す。54 分はこの TTL のわずかに内側なので、cache が warm なうちに compaction していると考えると整合する。これは推論で、機構は未確認である。

**確立していないこと:**

- **本当にその機能か。** 記録は `auto` で、機構は確認していない(`capture_experiments.md` でも未確認)。「約 1 時間」と 54 分の差、発火しない 3 割の理由も説明できていない。
- **mod の `precompute` との関係。** 型定義では `precompute` は結果を保持して次の compaction に使う経路である。mod は max に上げられる間これを skip する(799 行)。これが放置型と結びつくかの証拠はない。
- **どの effort で要約しているか。** compaction の要求は transcript に記録されない。mod の順序を通らず `auto` と記録されるので session effort と**推定**されるが、測定していない。
- **`autoCompactWindow` を下げたときの 200k 条件の変化。** 絶対値のまま残るのか、window に連動するのかは不明である。

**設計への意味:**

- 「観測された圧縮」を 870k の方針の結果として読んではいけない。現行の 870k に相当しうる手動は 1 件しかない。
- リプレイの結論はこれが続くかで変わる。Opus で 870k → 400k の回数増は、放置型ありで 111 → 142、なしで 45 → 107 である。
- 発動点 200k 以下のリプレイでは、放置型の発火は 0 になる。window 200,000(P=150k)ならほとんど起きなくなると見込まれる。400,000(P=330k)なら 3〜5 割が放置型のまま残る(300k 行で 57/174、400k 行で 73/142)。いずれもリプレイの値である。これは window の選択に関わる数少ない構造的な差である。

### advisor を使ったときの usage の合算

**確立していること:**

- transcript では、advisor を使うリクエスト(全 191、Opus 183)の usage のトップレベルが前後の model iteration を合算する。例: 実値 約 168k に対して表示 約 335k。未補正のピーク max は 1,922,147 で 1M を超える(直接測定)。`audit.md` も iterations が 2 以上の 174 組で同じことを独立に確認した。
- `register.ts` 614 行は `input + cache_read + cache_creation` をトップレベルで足す。
- plugin の型定義は、`ModelUsage` を「summed over the call's responses when it made several」と説明し、`turn.step` の `TurnUsage` も同じ形だと書く(`index.d.ts` 6588–6603 行)。これは `local_session_measurements.md` の「可能性」より強い、文書レベルの根拠である。
- 型定義は `SessionContextUsage.tokens` を「Input tokens the last response was answered over」と説明する(11491 行以降)。

**確立していないこと:** advisor の直後に mod が受け取る値の実測である。型定義の版は実行中の 2.1.304 と同じとは限らない。測定 M2 で 1 回記録すれば決まる。

**合算されている場合の影響(コードからの帰結):**

- `contextTokens`(859 行)が約 2 倍になり、`nearThreshold`(519 行)と `autoCompactDue`(523 行)が同じ値を比べる。**ターン終了時の compaction が実文脈の約半分で発火しうる。**
- 現行の 1M なら実 約 435k で発火しうる。window 200,000 なら実 約 75k、つまり**床の直後で発火しうる。** 閾値を下げるほど重大になるので、window を変える前に直す必要がある。
- ramp を残す場合は、次のターンの level も過大になる。

**修正案:** `completeMainTurn` はすでに `engine.session.usage` を呼んでいるので、`autoCompactDue` の判定を `context.tokens` にする。`nearThreshold` は事前判定なので過大でも害は呼び出しが増える程度で済む。`context.tokens` が最終 iteration の値かは M2 で同時に確認する。

---

## 6. 変更の順序(`register.ts`、編集はしていない)

段階ごとに判断できるよう分けた。

| 段階 | 変更 | 根拠の強さ | 主な箇所 |
|---|---|---|---|
| 1 | `index === 0` で決定して保持。表示は自動で一致。stuck note の文面修正。`autoCompactDue` を `context.tokens` に | 直接測定と文書 | 851–861、695–696、159–160、522–523 行 |
| 2 | M1 で確認後、`autoCompactWindow` を測定区間(200,000〜400,000)内に設定。同時に context ramp を外す | 下限は事実、上限と 1 点は M4 で未確定 | 169、279–285、577–578 行、settings |
| 3 | floor の引き上げを外し、失敗 round の数だけ残す | 間接証拠 | 170、287–307、322–323 行 |
| 4 | compaction を session effort にする(M5 で反証されなければ) | 不在の証拠と費用 | 171、452–542、793–835 行 |
| 付随 | README とコメントの根拠の修正。「Decide per step」「Raise with context」「Raise on failures」は根拠が適用できない。cache の Bedrock は docs によれば保たれない。Sonnet 5.5 の測定は手元の transcript で再現できない(`audit.md`、n=0)。`ui.render` の短縮は effort の議論と独立なので残してよい | 監査 | README |

段階 4 まで行うと `turn.step` のフックは不要になり、mod は stuck note、guideline、表示の短縮だけになる。削除で壊れるものとして、過去の版が `max` のまま残したセッションの復旧(`strandedAtCompactionLevel`)がある点に注意が必要である。

ターン終了時の先行 compaction(`PREEMPT_RATIO`)は、max を外すと元の目的(172–174 行)を失う。ただしターン終了時とターン途中の compaction を比較した研究がなく、事実では決まらない。既存コードであり、ターン途中の再読を減らす可能性があるので、**測定まで現状維持とする。** 0.9 は `shortcut: not tuned` のとおり未調整の値である。

---

## 7. 未検証の事項

**effort の動作**

- ターン途中で効かないことが、3 step 目以降、別の課題、別のバージョンでも同じか(現在は 1 種類の課題、各 5 回、2.1.304)。
- top-level と per-message の優先関係。top-level のみの変更で cache が無効になるか(n=0)。
- `context` 以外の方法(tool_result と同居する text、別の text block)でターン途中の変更を効かせられるか。**効くなら step ごとの設計を再検討できる。**
- `index === 0` が常に新しい入力の直後か。task-notification、キュー入力、予約実行、ターン途中の compaction の後はどうか。mod の「turn」がエンジンターンかユーザーターンかは README からの解釈である。
- medium、high、xhigh の間の出力量の差(low と max の両端しか測っていない)。
- **effort が回答の品質にどう効くか。** どの level の組み合わせでも未測定である。実験の回答(144 が 13 回、145 が 2 回)は正答も検証していない。
- Sonnet 5.5 での cache 維持(変更の組が 0 件)。Foundry の cache。

**compaction の trigger**

- 1M window を保ったまま window 200,000〜400,000 の高い閾値で、100,000 と同じ動作になるか。settings の `autoCompactWindow` が発動でも環境変数と同じ動作をするか(`/context` の値は確認済み)。
- 圧縮前 200k 未満の床の大きさ。実機の極小条件では 43.5k〜43.9k、完了後 24k だったが、ユーザー設定を外した環境である。
- 閾値を変えたときの Opus 5.5 の品質。**「medium で扱い切れる token 数」そのものが未測定である。**
- 閾値を下げたときの再読、作業分割、利用者行動の変化。
- 他モデル(Sonnet はリクエストの約 3 割)と subagent への波及。mod から設定を書けるか。
- 閾値式(33k)が将来の版でも保たれるか。`precomputeBufferFraction` などの別経路。
- ターン終了時とターン途中の compaction の品質比較(研究も見つからない)。

**compaction の effort と内容**

- max と session effort で下流の成功率と constraint 残存に差があるか。
- 出力の増分が thinking か可視の要約か。
- 放置型 compaction の effort と機構(5 章)。
- `guidedCompaction` の instruction が要約の入力にどう影響するか。API の「custom instructions は既定を置き換える」は `/compact` が API を使わないので当てはまらないが、Claude Code 側の挙動は未確認である。

**recovery**

- 次のターンへの floor の持ち越しに利益があるか。
- stuck note にモデルが従うか、従って結果が良くなるか。
- 失敗回数の適切な上限(比較研究なし)。

**測定と文献の限界**

- advisor 使用時に mod が受け取る usage の実値、および `context.tokens` の定義(5 章)。
- 1 利用者、約 33 日、少数セッションへの偏り。未分類の user 文字列 1,104 件のうち 312 件がプロンプトだった場合、ユーザーターン数は過小である。
- `audit.md` が未照合の論文(2609.32961、Bud、CliffCompaction、Slipstream、Complexity Trap、PredicateLongBench、Self-Debug、Reasoning Trap ほか)の数値。OpenAI の値は r.jina.ai 経由で、図の値は目視である。
- 文献検索は網羅的ではない。「見つからなかった」は「存在しない」ではない。Opus 5.5 の system card は読んでいない。
