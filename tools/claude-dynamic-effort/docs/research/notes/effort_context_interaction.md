# reasoning effort と context size の相互作用: 「context 増加に応じて medium → high → xhigh へ上げる」方針の検証

**結論:** この方針を一般則として支持する一次証拠はない。方針そのもの(context 量をトリガーに effort を段階的に上げる)を評価した研究は 0 件で、Claude の effort parameter や adaptive thinking を context 長と交差させた研究も 0 件である。effort と context 長を同時に報告する研究は 2 件あるが、結果は食い違う。[OOLONG](https://arxiv.org/abs/2511.02817) は長い context で high の利得が消えると報告する。[CL-bench](https://arxiv.org/abs/2602.03587) の著者は利得が拡大すると述べるが、同論文の数値では、絶対差(percentage point)は平均で拡大せず、相対比では 9 モデル中 5 モデルで拡大する(算出)。長い bin は solving rate が低く bin 別 n も不明で、ばらつきが大きい。各論の根拠を踏まえた本ノートの推論としては、context 量より step の難易度で effort を決めること、明らかに無関係な context は effort で補うより除去すること、xhigh は自分の eval で利得を測ってから使うことの方が支持が強い。

記法: 「**算出**」は論文の数値から本ノートで計算した値で、論文には記載がない。「**著者解釈**」は著者の仮説または解釈で、対照実験では検証されていない。

## Q1. reasoning effort / budget と context length を直接交差させた研究はあるか

### Takeaway
部分的にはある。ただし、対照された条件で API の effort parameter と複数段階の context 長を交差させたのは OOLONG(GPT-5-nano の 1 図)だけで、CL-bench は観察的な bin 分けである。Claude の effort、adaptive thinking、agent 環境、方針そのものを扱う交差実験は見つからなかった。

### Cited Findings

#### 交差の有無による分類
- **API effort × 複数段階の context 長(対照あり):** OOLONG-synth のみ。GPT-5-nano の high / low を 8K〜256K で比較した。high が有用なのは短い context だけで、64k 以降は差がほぼなく、256k では high が low をわずかに下回る。著者の仮説は、context window に余裕があるときだけ high がラベル列挙戦略を促すというもの(著者解釈)— [OOLONG](https://arxiv.org/abs/2511.02817)
- **effort / thinking の有無 × context 長 bin(観察的):** CL-bench。著者は「高 effort の優位は context が長いほど顕著」と述べる — [CL-bench](https://arxiv.org/abs/2602.03587)。ただし bin ごとにタスクが異なり、長さとタスクが交絡する。後述の再計算では平均的な拡大は確認できない。
- **prompt による max effort × context 有無(2×2):** Reasoning Shift。API parameter ではなく prompt による操作で、context も baseline / Long Input の二値である — [Reasoning Shift](https://arxiv.org/abs/2604.01161)
- **effort × 難易度軸(context 長は固定):** PredicateLongBench。context は synthetic で約 128K に固定されており、長さとの交差ではない — [PredicateLongBench](https://arxiv.org/abs/2607.08284)
- **モデル間比較のみ:** o4-mini と GPT-4.1 の差。同一モデルで effort を変えていない — [WebAgents](https://arxiv.org/abs/2512.04307)

#### 横断比較表

| 研究 | モデル | タスク | 指標 | 効果量(一次資料で確認) | サンプルサイズ | effort × context |
|---|---|---|---|---|---|---|
| [OOLONG](https://arxiv.org/abs/2511.02817) | GPT-5-nano(ほか多数) | 長文の集計・分類(synth / real) | score | high の優位は 64k までで消失、256k で high < low。real では差がごく小さい。数値表はなく図のみ | 長さごとに 400 問(synth) | **あり**(API、対照あり) |
| [CL-bench](https://arxiv.org/abs/2602.03587) | 9 モデル(Fig.16) | context learning | solving rate | GPT-5.1 全体で 21.2% → 23.7%。bin 別の差は下表 | 500 contexts / 1,899 tasks。bin 別 n は未確認 | **あり**(観察的) |
| [Reasoning Shift](https://arxiv.org/abs/2604.01161) | Qwen3.5-27B、Gemma 4 31B、gpt-oss-120b、Gemini 3 Flash Preview、Kimi K2 Thinking | IMOAnswerBench、GPQA-Diamond | 正答率、reasoning tokens | 非 baseline 条件で tokens 最大 74% 減。Long Input の IMO 正答率は Qwen 74.5 → 67.8、gpt-oss 73.8 → 64.0 | IMO 400 問、GPQA は 2 run 平均、難易度分析は 30 問 × 50 run | prompt で 2×2 |
| [PredicateLongBench](https://arxiv.org/abs/2607.08284) | GPT 5.4、Opus 4.6、Gemini 3.1 Pro Preview ほか | 長文内の部分列探索 | 正答率 | 探索空間拡大で GPT 5.4 high は 92% → 10%(context 128K で一定)。Opus 4.6 high は decoy ありの unary で 7% | variant ごとに 100 例(一部 93) | なし(長さ固定) |
| [WebAgents](https://arxiv.org/abs/2512.04307) | Claude-3.7(thinking なし)、GPT-4.1、Llama 4、o4-mini | 実 Web の依存 subtask | 成功率 | No Noise → 150k: o4-mini 46.2 → 21.2%、GPT-4.1 44.4 → 2.7% | 基礎タスク 56 件 | なし(モデル間) |
| [ARES](https://arxiv.org/abs/2603.07915) | gpt-oss-20b(router は Qwen3-1.7B) | TAU-Bench、BrowseComp-Plus、WebArena | 成功率等、reasoning tokens | Q3 の表を参照 | テスト件数の明記は未確認。算出では約 115 / 50 / 150 / 129 タスク | なし |
| [Inverse Scaling](https://arxiv.org/abs/2507.14417) | Claude Opus 4、o3、DeepSeek R1 ほか | distractor 付き計数など | 正答率、RMSE | R1 は 5 distractor の Misleading Math で 70 → 30%(natural)。Opus 4 は Misleading Python で近完全 → 約 80%(controlled) | 各 2,500 問、budget 条件 3 反復 / natural 5 反復 | distractor 数 × budget(長文ではない) |
| [Mirage](https://arxiv.org/abs/2506.04210) | DeepSeek-R1-Distill 系 3 種 | GSM-8K、MATH-500、AIME 2024 | 正答率 | 思考 385 → 1,100 tokens で 82.2 → 87.3%、15,980 tokens で 70.3%(Fig.2(a)) | 1,320 / 500 / 30 問 | なし |
| [When More Thinking Hurts](https://arxiv.org/abs/2604.10739) | DeepSeek-R1-32B、s1-32B | AIME、MATH-500、GPQA Diamond | 正答率、marginal utility | R1 は 500 tokens で 28.2%、12K で 55.8%、16K で 54.9% | AIME 60 問、GPQA 198 問 | なし |
| [Overthinking (Cuadron)](https://arxiv.org/abs/2502.08235) | o1 high / low ほか | SWE-bench Verified | issue 解決率、費用 | o1 high 29.1% / $1,400、low 21.0% / $400、low × 2 本から選択 27.3% / $800 | 4,018 軌跡。instance 数は未確認 | なし |
| [Chen](https://arxiv.org/abs/2412.21187) | QwQ-32B-Preview、DeepSeek-R1 | GSM8K、MATH500、GPQA、AIME ほか | tokens、効率指標 | 2+3 系の問題で 1,953% 多い tokens、初回解答で正解が 92% 超、MATH500 で 48.6% 削減かつ正答率維持 | test set ごとに異なる(未集計) | なし |
| [AdaptThink](https://arxiv.org/abs/2505.13417) | DeepSeek-R1-Distill-Qwen-1.5B | GSM8K、MATH500、AIME2024 | 正答率、応答長 | 平均で長さ -53%、正答率 +2.4 | 3 データセット。件数は本文で未確認 | なし |
| [TALE](https://arxiv.org/abs/2412.18547) | GPT-4o-mini ほか | 算術・数学ベンチ | 正答率、output tokens | TALE-EP は 83.75 → 81.03%、461.25 → 148.72 tokens(67% 減) | 6 ベンチ。件数は未集計 | なし |
| [DEER](https://arxiv.org/abs/2504.15895) | 11 モデル | 10 ベンチ | CoT 長、正答率 | 長さ 19.1〜80.1% 減、正答率 0.3〜5.0 改善 | 10 ベンチ × 11 モデル | なし |
| [Snell](https://arxiv.org/abs/2408.03314) | PaLM 2-S* 系 | MATH | compute 効率 | 最適配分で best-of-N の 4 倍超の効率。約 14 倍大きいモデルと FLOPs 揃えで比較 | MATH。件数は未確認 | なし(revision / search で thinking budget ではない) |
| [CatAttack](https://arxiv.org/abs/2503.01781) | DeepSeek R1、o1、o3-mini ほか | 短い無関係 trigger 付き数学 | 誤答率、応答長 | R1 の誤答 1.50% → 4.50%。o1 の 1.5 倍 slowdown 率 26.4%、R1 28.0% | 分析 225 問、大規模検証 1,000 問 | なし(短文) |
| [MazeBench](https://arxiv.org/abs/2603.26839) | GPT-5.4、GPT-5.5(全 19 構成の一部) | 迷路画像 | solve rate | none → low → medium: GPT-5.4 12 → 85 → 91%、GPT-5.5 15 → 92 → 96% | 110 迷路(core 100) | なし |
| [Du et al.](https://arxiv.org/abs/2510.05381) | 複数 | RULER ほか | 性能低下 | 完全な検索でも入力長増加で 13.9〜85% 低下(abstract のみ確認) | 未確認 | abstract に effort の言及なし |
| [Context Discipline](https://arxiv.org/abs/2601.11564) | Llama-3.1-70B、Qwen1.5-14B | 事実質問 + 無関係 context | 正答率、latency | 15,000 words で正答率 98.5 → 98%、99 → 97.5%、latency +719.64% | 200 問 | なし。非 reasoning、関連性は低い |

#### CL-bench Fig.16(context length 別 solving rate、high / low)
| モデル | 操作の種類 | 0-4K | 32K+ | 差の変化(算出) |
|---|---|---|---|---|
| GPT 5.1 | API effort | 30.2 / 29.3 | 16.2 / 8.8 | +0.9 → +7.4 |
| GPT 5.2 | API effort | 26.0 / 27.4 | 6.6 / 8.1 | -1.4 → -1.5 |
| Gemini 3 Pro | API effort | 21.8 / 19.8 | 8.8 / 8.8 | +2.0 → 0.0 |
| Claude Opus 4.5 | thinking あり / なし | 29.0 / 29.1 | 8.1 / 6.6 | -0.1 → +1.5 |
| Kimi K2 | variant | 24.2 / 17.3 | 9.6 / 3.7 | +6.9 → +5.9 |
| HY 2.0 | variant | 27.0 / 23.3 | 5.9 / 5.1 | +3.7 → +0.8 |
| Qwen 3 Max | variant | 19.8 / 21.7 | 5.1 / 4.4 | -1.9 → +0.7 |
| DeepSeek V3.2 | variant | 18.4 / 18.6 | 6.6 / 3.7 | -0.2 → +2.9 |
| Doubao 1.6 | variant | 21.7 / 18.0 | 2.9 / 8.1 | +3.7 → -5.2 |

出典: [CL-bench](https://arxiv.org/abs/2602.03587) Fig.16(図の数値ラベルを目視確認)。操作の種類は同論文 Appendix C による。

- 算出(絶対差): 9 モデルの差の単純平均は 0-4K で +1.5、32K+ で +1.4 である。拡大 4 モデル(GPT 5.1、Opus 4.5、Qwen 3 Max、DeepSeek V3.2)、縮小 4 モデル(Gemini 3 Pro、Kimi K2、HY 2.0、Doubao 1.6)、ほぼ不変 1 モデル(GPT 5.2)に分かれる。
- 算出(相対比): 9 モデル平均の high / low は 0-4K で約 1.07、32K+ で約 1.22 である。モデル別では拡大 5(GPT 5.1、Opus 4.5、Kimi K2、Qwen 3 Max、DeepSeek V3.2)、ほぼ不変 1(HY 2.0)、縮小 3(Gemini 3 Pro、GPT 5.2、Doubao 1.6)に分かれる。
- 真の API effort 比較 3 モデルでは、どちらの指標でも拡大するのは GPT 5.1 だけである。

### Inferences
- OOLONG は方針に反する。CL-bench の著者の記述は方針に沿い、相対比では多くのモデルで裏づけられるが、絶対差では拡大せず、真の API effort 比較では GPT 5.1 に依存する。長さの効果か指標の性質かを分ける有意差検定や bin 別 n も確認できない。どちらかを正解として選べる材料はない。
- 両者の差の一因として、タスク性質(情報密度の高い集計 vs 新規知識の学習)、モデル規模(nano vs 上位モデル)、対照の有無が考えられる。いずれも未検証である。
- context の「長さ」だけでは条件を特定できない。関連情報か無関係情報かで結果が変わる可能性が高い(Q2)。

### Gaps

#### 一次資料の確認状況
- **指定 9 件はすべて実在し、タイトルも一致した:** 2603.07915(ARES)、2408.03314(Snell)、2505.13417(AdaptThink)、2502.08235(Overthinking)、2412.21187(Chen)、2507.14417(Inverse Scaling)、2412.18547(TALE)、2504.15895(DEER)、2506.04210(Mirage)。
- **追加で本文を確認:** 2604.01161、2511.02817、2602.03587、2607.08284、2512.04307、2604.10739、2503.01781、2601.11564、2603.26839。2510.05381 は abstract のみ確認した。
- **確認できず採用しなかった主張:** 16x engineer ブログの「Claude Sonnet 4 (Thinking) は 60k〜120k で低下」、Morph による Chroma 研究の要約。いずれも二次情報である。
- **論文内の不整合:**
  - CL-bench 本文の「Opus 4.5 は 0-15K と 120K+ の間で 20% 超低下」は、同論文の入力長最大 65.0K および最上位 bin 32K+ と整合しないため採用しなかった。
  - CL-bench の Fig.7(input length 別)と Fig.16(context length 別)は数値がわずかに異なる。本ノートは Fig.16 を使用した。
  - ARES abstract の「up to 52.7%」は Retail RL(476k vs 1007k)に対応するが、Airline RL は 133k vs 873k で算出 84.8% 減となり、この値を超える。
  - ARES abstract は Random に「meaningful cost reduction」がないとするが、Table 1 の Retail では High の約 36%(359k vs 1007k、算出)の tokens で済む。精度を維持できない点が問題である。
  - TALE-EP は本文で 80.22% / 138.53 tokens、表の Average 行で 81.03% / 148.72 tokens と異なる。本ノートは表の値を使用した。
  - Mirage の並列思考の優位は abstract で「up to 20%」、本文で 16K budget 時「up to 22%」である。
  - Overthinking の「almost 30%」と「43%」は基準が異なる可能性がある。算出では 27.3 / 21.0 = 1.30 で low 比、43% は本文では high 比の費用削減である。
- **未確認:** OOLONG Fig.3a の数値(表がないため記載しない)、CL-bench の bin 別 n と有意差。

## Q2. reasoning を増やすと長文脈の劣化は緩和されるか、悪化するか

### Takeaway
どちらも起こり、context の質とタスクに依存する。無関係な context はモデル自身の reasoning を短くし検証を減らすが、追加 reasoning による回復は部分的で、context を除去した場合より効果が小さい。短い distractor では reasoning を増やすと悪化するモデルもある。

### Cited Findings
- **無関係 context で reasoning は短くなる:** Long Input の IMO では全モデルで有意に短縮した(p < 10^-10)。論文の「最大 74%」に当たるのは Table 1 の Gemini 3 Flash Preview、GPQA、**Multi-turn** で、12,927 → 3,334 tokens(算出 74.2% 減)である。この条件の正答率は 89.6 → 88.7% とほぼ変わらず、短縮が常に有害とは限らない。Qwen3.5-27B は数百 tokens の挿入で 18% 減、さらに増やすと 50% 減。MATH-500 では 8,003 → 3,762 tokens(53% 減)で、non-thinking の応答長は 19% 減にとどまる — [Reasoning Shift](https://arxiv.org/abs/2604.01161)
- **減るのは主に検証部分である:** 最初の候補解までは -12.4% だが、候補解以降は -36.1%(Qwen3-32B、Long Input)。ほぼ完了した trace の再サンプルでの終了率は Baseline 21%、Long Input(16k) 46%。自信スコア最高の割合も Baseline context 62.0% から 16k context で 90.4% に上がる — [Reasoning Shift](https://arxiv.org/abs/2604.01161)
- **劣化は難問に集中する:** 正答率の低下は medium / hard に偏り、著者は easy では比較的無害とする — [Reasoning Shift](https://arxiv.org/abs/2604.01161)
- **強制的に考えさせても部分回復にとどまり、context 除去の方が効く(IMO、Table 5):** — [Reasoning Shift](https://arxiv.org/abs/2604.01161)

| モデル | Long Input | "Wait"(context 維持) | "Wait"(context 除去) | Baseline |
|---|---|---|---|---|
| Gemma 4 31B | 67.0% | 68.3% | 71.8% | 72.7% |
| Qwen3.5-27B | 67.8% | 68.5% | 69.3% | 74.5% |

- **max effort prompt の効果は一貫しない(Long Input の IMO):** Qwen 67.8 → 70.5%、Gemma 67.0 → 70.9%、gpt-oss 64.0 → 61.8%。tokens は baseline で 10〜15% 増にとどまり、短縮率は維持された(gpt-oss はどちらも 51%)。著者は prompt だけでは不十分と結論する — [Reasoning Shift](https://arxiv.org/abs/2604.01161)
- **fine-tuning は tokens を戻すが正答率は戻しきれない:** Olmo-3-7B-Think-SFT は Long Input(32) で 1,908 → 2,806 tokens、正答率は 91.4 → 93.8% だが baseline 96.1% には届かない — [Reasoning Shift](https://arxiv.org/abs/2604.01161)
- **distractor では悪化しうる:** R1 は 5 distractor で 70 → 30%、Opus 4 は Misleading Python で近完全 → 約 80%。一方 o3 は Python で正の scaling を示す — [Inverse Scaling](https://arxiv.org/abs/2507.14417)
- **短い無関係文で誤答と長さが増える:** 誤答可能性は reasoning model で最大 500%、instruction-tuned で最大 700% 増。R1 の 2 倍 slowdown 率は 16.2% — [CatAttack](https://arxiv.org/abs/2503.01781)
- **関連情報が密な長文では reasoning は長くなる:** Gemini 3 Pro は入力が長いほど trace が長く、入力長を固定すると trace 長は正答と負に相関する(著者は難易度の交絡を指摘)。DeepSeek-R1 と Qwen-3-4B は上限まで reasoning して回答できない over-reasoning を示す — [OOLONG](https://arxiv.org/abs/2511.02817)
- **budget 増は一部のみ改善する:** GPT-5.4 high で Lex. locate は 80〜90 台まで伸びるが、Near-sorted scattered と Multi-list はほぼ動かず、著者は「Budget alone does not close the decoy gap」とする。Opus 4.6 を 16K → 64K にした 5 問でも、一部は依然 budget 切れ、他は 50K〜60K tokens 生成後に回答を拒否した — [PredicateLongBench](https://arxiv.org/abs/2607.08284)
- **長文脈では reasoning モデルの低下が小さい例もある:** 150k で o4-mini 21.2%、GPT-4.1 2.7%。ただしモデル間比較である。要約を挟む implicit RAG は小幅に改善した — [WebAgents](https://arxiv.org/abs/2512.04307)

### Inferences
- 無関係な履歴が原因の劣化には、effort を上げる前に compaction や除去を試す根拠がある。Table 6 では 3 モデルすべてで、標準 prompt の Baseline が max effort prompt の Long Input を上回る(74.5 vs 70.5、72.7 vs 70.9、73.8 vs 61.8)。Table 5 は同じ trace で context だけを変えた比較だが、常に "Wait" を挿入しており、除去の効果を単独では示さない。
- ただし挿入された context は純粋に無関係な Shakespeare なので、除去に損失がない。実際の compaction は関連情報も失いうるため、この差は compaction の利得の上限と見るべきである。
- 「モデルが自然に考える量が減る」ことは「effort を上げれば戻る」ことを意味しない。不足しているのは token 量だけではなく、検証の発火と効き目である可能性がある。
- 関連情報が密な context では逆に、上限超過による無回答が主なリスクになるため、effort より `max_tokens` や残り context の確保が先に問題になる可能性がある。

### Gaps
- 実 agent 履歴での再現は未確認である。Reasoning Shift は条件が synthetic であることを limitation として明記し、Multi-turn も第 2 turn のみを評価している。
- compaction と effort 引き上げを同一タスクで直接比較した研究は見つからなかった。

## Q3. per-step effort 選択と固定 effort の比較(精度とコスト)

### Takeaway
per-step 選択は固定 high と同等の精度で reasoning tokens を大きく減らせる。ただし固定 medium が優位な場面もあり、「常に上げる」より「step ごとに必要な分だけ」を支持する結果である。どの研究も context 長を選択基準として評価していない。

### Cited Findings
ARES の主要結果(gpt-oss-20b。Ttotal は reasoning tokens 総計)— [ARES](https://arxiv.org/abs/2603.07915)

| 環境 | Low | Medium | High | ARES SFT | ARES RL |
|---|---|---|---|---|---|
| TAU Retail | 35.0 / 25k | 47.3 / 137k | 54.8 / 1007k | 54.8 / 652k | 58.5 / 476k |
| TAU Airline | 32.0 / 12k | 42.0 / 98k | 38.0 / 873k | 36.0 / 678k | 42.0 / 133k |
| BrowseComp-Plus | 8.00 / 5k | 34.0 / 538k | 42.7 / 1841k | 41.3 / 1071k | 未実施 |
| WebArena | 37.4 / 67k | 42.6 / 538k | 45.0 / 2763k | 46.5 / 1512k | 未実施 |

- SFT の削減率は固定 high 比で Retail 35.2%、BrowseComp 41.8%、WebArena 45.3% である — [ARES](https://arxiv.org/abs/2603.07915)
- Random は 4 環境すべてで ARES SFT より tokens が少ないが、精度は劣る(Retail 43.5 / 359k、ARES SFT 54.8 / 652k)。GPT-5 による prompt ベースの選択も Retail で精度 40.0 と router に劣る — [ARES](https://arxiv.org/abs/2603.07915)
- **Airline では固定 medium が固定 high に勝つ**(42.0 vs 38.0)。著者も overthinking と呼ぶ。さらに算出では、固定 medium は ARES RL と同精度で tokens が少ない(98k vs 133k)— [ARES](https://arxiv.org/abs/2603.07915)
- gpt-oss-120b への転用では ARES 65.2% に対し High 67.8%、tokens は約 23% 減 — [ARES](https://arxiv.org/abs/2603.07915)
- **方針に最も近い観察:** WebArena では序盤(step 0–2)は low が多く、進行とともに high が増える。著者はこれを observation の複雑化と context window の拡大に帰している(著者解釈)。high は `go back` と `branch` の行動に集中する — [ARES](https://arxiv.org/abs/2603.07915)
- ARES は effort 切替で KV cache を再利用できる点を、model routing に対する利点として挙げる — [ARES](https://arxiv.org/abs/2603.07915)
- SWE-bench Verified では、o1 low を 2 本生成して選ぶ方法で 27.3% / $800 となり、high 1 本(29.1% / $1,400)より 43% 安い。3 本では 30.3% で 15% 安い。選択は overthinking スコアによる事後選択である — [Overthinking](https://arxiv.org/abs/2502.08235)
- 問題単位の適応では、AdaptThink の NoThinking 率は GSM8K 86.9%、MATH500 76.8%、AIME 40.4% で、正答率 +2.4、長さ -53% — [AdaptThink](https://arxiv.org/abs/2505.13417)
- Snell は難易度に応じた compute 配分で best-of-N の 4 倍超の効率を報告する。難易度の推定には oracle 的な評価が前提となる — [Snell](https://arxiv.org/abs/2408.03314)

### Inferences
- ARES の step 別傾向は方針の唯一の直接的な手掛かりだが、step 番号、ページの複雑さ、context 量が交絡しており、context が原因とは言えない。また high が `go back` / `branch` に集中する結果は、単調な引き上げより行動種別に応じた選択を示唆する。
- 固定 high を基準にした削減率は、固定 medium との比較では小さくなるか逆転する。per-step 導入の前に固定 medium を baseline に含めるべきである。

### Gaps
- ARES の結果は gpt-oss 系のみで、router 自身の計算費用が token 指標に含まれるかは本文から確認できなかった。RL は TAU-Bench のみである。
- context 量をトリガーとする policy と step 難易度 router を比較した研究は見つからなかった。
- prompt cache 費用を含めて effort 切替を評価した研究は見つからなかった。

## Q4. reasoning budget と精度の dose-response

### Takeaway
多くの研究で、上昇 → 飽和 → 場合により低下という非単調な形が見られる。ピークは難易度に依存し、簡単な問題ほど早い。ただし難しい探索タスクでは budget が大きいほど伸び続けるものもあり、全タスク共通の最適点はない。

### Cited Findings
- **上昇後の低下:** 82.2% → 87.3%(385 → 1,100 tokens)→ 70.3%(15,980 tokens)。著者は分散の増加で説明する。exact budget では増やすほど低下した — [Mirage](https://arxiv.org/abs/2506.04210)
- **逓減と負転:** R1-32B の marginal utility(500 tokens あたり)は 0.5–2K で +3.2、8–12K で +0.1、12–16K で -0.3。s1 は 8–12K で -0.2 — [When More Thinking Hurts](https://arxiv.org/abs/2604.10739)
- **難易度依存:** 精度ピークは Level 1–2 で約 1.5K、Level 5 は約 8K まで利得がある。overthinking の閾値は 2K vs 8K — [When More Thinking Hurts](https://arxiv.org/abs/2604.10739)
- **飽和:** GPT-5.4 は 12% → 85% → 91%、GPT-5.5 は 15% → 92% → 96%(none → low → medium、迷路タスク)— [MazeBench](https://arxiv.org/abs/2603.26839)
- **タスクで形が異なる:** GPT-5.4 high で Unary sparse は 16K で飽和、Lex. locate は 80〜90 台まで伸び、Near-sorted scattered と Multi-list はほぼ動かない — [PredicateLongBench](https://arxiv.org/abs/2607.08284)
- **モデル差:** Opus 4.6 は no reasoning でも lex-locate で 25–35% を保つが、GPT-5.4 は 5% 以下に落ちる — [PredicateLongBench](https://arxiv.org/abs/2607.08284)
- **推定の難しさ:** 最初の解答で正解が 92% 超 — [Chen](https://arxiv.org/abs/2412.21187)。early exit で CoT 長 19.1〜80.1% 減、正答率 0.3〜5.0 改善 — [DEER](https://arxiv.org/abs/2504.15895)
- **逆に削りすぎの費用:** TALE-EP は tokens 67% 減で正答率は 83.75 → 81.03% — [TALE](https://arxiv.org/abs/2412.18547)。NoThinking は AIME で 29.4 → 14.0% に落ちる — [AdaptThink](https://arxiv.org/abs/2505.13417)
- **inverse scaling はタスクとモデルに依存:** regression では Sonnet 3.7、Opus 4、o3-mini が controlled で一貫した inverse scaling を示す一方、Qwen3-32B は正の scaling を示す。natural では多くが noisy または flat — [Inverse Scaling](https://arxiv.org/abs/2507.14417)

### Inferences
- medium → high で伸びるタスクでも、high → xhigh で伸びる保証はない。上記の逓減と負転は、上位段の利得が最も不確実であることを示す。
- 難易度がピークを決めるという結果は、context 量より難易度を effort の入力にすべきという判断の根拠になる。ただし長い context で難易度自体が上がる(CL-bench の著者の見方)場合、両者は相関する。

### Gaps
- 多くの dose-response は数学系で小規模(AIME 30〜60 問)で、budget forcing による人工的な延長を含む。自然に長く考えた場合と同一視はできない。
- context 長ごとのピーク位置を測定した研究は見つからなかった。

## Q5. adaptive thinking + effort parameter のモデルに適用できるか

### Takeaway
直接は適用できない。Claude Opus 5.5 などでは `budget_tokens` が拒否され、effort は厳密な budget ではない行動シグナルである。最新世代での外部検証は見つからず、固定 budget の研究は方向性の仮説にとどまる。特に tool loop では effort を上げても thinking が増えるとは限らない。

### Cited Findings
- **仕組みが異なる:** `budget_tokens` は Claude Opus 5.5、Sonnet 5.5、Haiku 5.5、Fable 5.1、Opus 5 などで 400 error。Opus 4.6 / Sonnet 4.6 では deprecated — [Anthropic Thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- **effort はシグナル:** 「Effort is a behavioral signal, not a strict token budget.」thinking だけでなく text や tool call を含む全 token に影響する — [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)
- **モデル自身の傾向と文書が一致:** 「Thinking per request also tends to decrease as a conversation grows longer.」— [Anthropic Steering thinking](https://platform.claude.com/docs/en/build-with-claude/thinking-steering-and-cost)。方向は Reasoning Shift と同じである。
- **tool loop では上げても効かない場合がある:** thinking の多くは新しい user 入力直後の request に集中し、tool 結果処理のみの request は `xhigh` や `max` でも thinking を省略しうる — [Anthropic Steering thinking](https://platform.claude.com/docs/en/build-with-claude/thinking-steering-and-cost)
- **level 名はモデル間で等価ではない:** Opus 5.5 の `medium` は Anthropic のテストで Opus 5 の `high` と同等以上で、複数の coding eval では `low` も近い。既定は `medium` — [Prompting Claude Opus 5.5](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5)。Sonnet 5.5 も level が再調整されており、再 sweep が推奨される — [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)
- **上位 level のガイダンス:** Opus 5.5 では「Reserve `xhigh` and `max` for work where you've measured a quality gain.」— [Prompting Claude Opus 5.5](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5)。`xhigh` の典型用途は 30 分超かつ token budget が数百万規模の長時間タスクで、基準は実行規模であり context 占有量ではない — [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)。OpenAI も `xhigh` は「Only use when your evals show a clear benefit」とする — [OpenAI Reasoning](https://developers.openai.com/api/docs/guides/reasoning)
- **切替のコスト:** top-level effort の変更は prompt cache を無効化する。per-message effort(beta)は対応モデルで cache を維持し、Anthropic はこれを勧める — [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)
- **effort は context 増加を加速しうる:** 高 effort は tool call が増える傾向がある — [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)。keep-all モデルでは過去の thinking も input として残る — [Anthropic Thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- **公式の動的 effort の基準は複雑さ:** 「Adjust effort based on task complexity.」であり context size への言及はない — [Anthropic Effort](https://platform.claude.com/docs/en/build-with-claude/effort)。OpenAI の文書も context window については reasoning 用の余地確保(まず 25,000 tokens 以上)を述べるのみで、context 長に応じた effort 選択は勧めていない — [OpenAI Reasoning](https://developers.openai.com/api/docs/guides/reasoning)
- **研究に含まれる Claude の条件:** Opus 4(prompt keyword + budget)— [Inverse Scaling](https://arxiv.org/abs/2507.14417)、Opus 4.5(thinking あり / なし)— [CL-bench](https://arxiv.org/abs/2602.03587)、Opus 4.6(thinking budget 12K / 0)— [PredicateLongBench](https://arxiv.org/abs/2607.08284)、Claude-3.7(thinking なし)— [WebAgents](https://arxiv.org/abs/2512.04307)。いずれも effort parameter の level 比較ではない。
- **xhigh の外部データは事実上ない:** 確認できた唯一の記載は Reasoning Shift の gpt-oss-120b で、Together AI 経由で reasoning effort `xhigh` とある。Long Input で tokens 51% 減、IMO 正答率 73.8 → 64.0% だが、medium / high との比較はない — [Reasoning Shift](https://arxiv.org/abs/2604.01161)。ARES は gpt-oss の level を high / medium / low と記述しており — [ARES](https://arxiv.org/abs/2603.07915)、この `xhigh` の実体は不明である。Anthropic や OpenAI の `xhigh` の証拠としては扱えない。

### Inferences
- 移転しやすいもの: 無関係 context が検証を弱めるという行動傾向(Anthropic の文書の記述と整合)、難易度依存、per-step 選択の考え方。
- 移転しにくいもの: tokens を強制する dose-response の数値、level 名に紐づく閾値、Inverse Scaling の Opus 4 の結果(adaptive 以前の世代で prompt 操作を使用)。
- ARES の削減は effort 切替時の KV cache 再利用を前提とする。Claude でこれが成り立つのは per-message effort(beta)の場合だけで、top-level effort の変更では cache が無効になり、その費用は ARES の計算に含まれない。
- 長い context での top-level effort 変更は、cache 再作成の対象が大きくなるため費用が context とともに増える(機構からの推論で、実測ではない)。方針を試すなら per-message effort が前提になる。
- 上げた effort が thinking に反映されるかは、`usage.output_tokens_details.thinking_tokens` で確認できる — [Anthropic Steering thinking](https://platform.claude.com/docs/en/build-with-claude/thinking-steering-and-cost)。方針の検証には、自分の eval で context 量(無関係 / 関連)× effort(medium / high / xhigh)× compaction 有無を同一タスクで比較するのが最小の実験となる。

### Gaps
- Opus 5 / 5.5、Sonnet 5 / 5.5 世代で effort と context 長を交差させた外部研究は見つからなかった。Anthropic の比較は定性的な自社テストで、context 長別の数値はない。
- Reasoning Shift に Claude は含まれない。Claude での短縮の大きさと、それが精度低下を伴うかは不明である。
- API の effort parameter が、prompt の max effort と異なり短縮を打ち消せるかは未検証である。これが方針の成否を分ける最大の未解決点である。
