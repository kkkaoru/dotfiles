# LLM・coding agent の性能は context 量でどう劣化するか(横断比較)

凡例と検証方法:

- 調査日は 2026-11-05。
- 「原文照合」は、PDF または arXiv HTML の生テキストと図を直接確認した数値を指す。
- 「要約経由」は、一次ソースを WebFetch の要約モデル経由で読んだ数値を指す。生テキストの抜き取り照合を行った論文もあるが、全数値を照合したわけではない。
- 表の「窓比」は、筆者が k=1000 として計算した値で、各ソースの記載値ではない。

## どの絶対トークン数・窓比から劣化が現れるか

### Takeaway

普遍的な閾値は存在しない。同じ「性能低下」でも、劣化点は数百トークン(GSM-IC、distractor 1 文)から 256K 超(GPT-5.5 の MRCR v2)まで 3 桁近くばらつき、モデル世代・タスク・filler で決まる。2026 世代の検索系ベンチマークで大きな低下が明瞭になるのは 256K 付近より上だが、GPT-5.4 では 32K-64K 帯(97.2→90.5)からすでに緩やかに下がり、coding agent では約 30 万文字で失敗率が上がる例と上がらない例が併存する。

### Cited Findings

#### 横断比較表

| 研究 | モデル | タスク | 指標 | 効果量 | サンプル | 劣化の出現点 | 窓比(筆者計算) | Source |
|---|---|---|---|---|---|---|---|---|
| Lost in the Middle: How Language Models Use Long Contexts (2023) | GPT-3.5-Turbo、Claude-1.3、MPT-30B-Instruct、LongChat-13B とその拡張窓版 | Multi-document QA、key-value retrieval | Accuracy | GPT-3.5-Turbo は位置により 20% 超低下。20 文書では index 0 が 75.8%、index 9 が 53.8% | QA 2655 query | 10/20/30 文書(平均 1,476/2,946/4,419 tokens)で位置依存を観測 | 16K 版で約 9〜28%(平均 token 基準) | [arXiv:2307.03172](https://arxiv.org/abs/2307.03172) |
| Large Language Models Can Be Easily Distracted by Irrelevant Context (GSM-IC, 2023) | code-davinci-002、text-davinci-003 | 算数問題に無関係文を 1 文追加 | Micro / Macro accuracy | COT の base 95.0 に対し GSM-IC-4K は micro 72.4、macro 6.0(code-davinci-002) | 4,000 問(全 58,052 問から抽出、base 100 問) | 無関係文 1 文。長さではなく内容の効果 | 算出不能 | [arXiv:2302.00093](https://arxiv.org/abs/2302.00093) |
| RULER: What's the Real Context Size of Your Long-Context Language Models? (2024) | 17 モデル | 13 タスク | 論文内の各スコア | abstract は「32K 以上を謳うモデルの半数しか 32K で満足な性能を維持できない」と記載 | 未確認 | 32K | モデル別は未確認 | [arXiv:2404.06654](https://arxiv.org/abs/2404.06654) |
| NoLiMa: Long-Context Evaluation Beyond Literal Matching (2025) | 主表 13 モデル(GPT-4o、Claude 3.5 Sonnet など) | 字句一致のない needle の検索 | Base score と 32K score | GPT-4o 99.3→69.7、Claude 3.5 Sonnet 87.5→29.8、Command R+ 90.9→7.4 | 長さごとに 7,540 テスト | 32K で 13 モデル中 11 が base の半分以下(より短い帯の値は未転記) | GPT-4o(128K 窓)の 32K は 25% | [arXiv:2502.05167](https://arxiv.org/html/2502.05167v3) |
| Context Rot: How Increasing Input Tokens Impacts LLM Performance (Chroma, 2025-07) | 18 モデル(Claude Opus 4、GPT-4.1、Gemini 2.5 Pro、Qwen3 など) | NIAH 変形、LongMemEval、Repeated Words | 判定器スコア | 本文に集計値なし(図のみ) | 194,480 call、LongMemEval 306 prompt | **未記載**。token 数の閾値を本文で示していない | 未記載 | [Chroma](https://www.trychroma.com/research/context-rot) |
| LongCodeBench: Evaluating Coding LLMs at 1M Context Windows (2025) | GPT-4.1、Gemini 2.5 Pro、Claude 3.5 Sonnet ほか | LongCodeQA、LongSWE-Bench | Accuracy、解決率 | Claude 3.5 Sonnet の解決率 29%→3%(32K→256K)。Qwen2.5 の QA 70.2%→40.0%(512K→1M) | QA 計 443 件(帯ごと 47〜113)、SWE 計 600 件(帯ごと 100) | SWE は 64K から低下(Claude 3.5 Sonnet)。QA は多くのモデルで 1M まで明確な低下なし | モデル別窓は未確認 | [arXiv:2505.07897](https://arxiv.org/html/2505.07897) |
| A Controllable Examination for Long-Context Language Models (LongBioBench, 2025) | 18 モデル(Fig. 9 は 16 モデル) | 検索、計算、rank など | Accuracy | Qwen2.5-7B-1M の understanding は 256k で 48.0、512k で 0.3 | 各テスト 800 件 | 2k→128k で多くが低下。1M 版は 128k までは維持 | 7B(131,072 窓)の 128k は 98%。1M 版(1,010,000 窓)の 512k は 51% | [arXiv:2506.02921](https://arxiv.org/html/2506.02921v2) |
| How Much Do LLMs Hallucinate in Document Q&A Scenarios? A 172-Billion-Token Study Across Temperatures, Context Lengths, and Hardware Platforms (Roig, 2026-03) | open-weight 35 モデル(32K→128K 比較は 26 モデル) | 文書 QA と fabrication probe | Accuracy など | 32K→128K の低下は中央値 10.4pp、平均 12.4pp(GLM 4.7 の 3.2pp〜Llama 3.3 70B の 28.3pp) | 4,264 run(32K 1,856 / 128K 1,480 / 200K 928) | 32K→128K | 未記載 | [arXiv:2603.08274](https://arxiv.org/html/2603.08274) |
| Understanding LLM Performance Degradation in Multi-Instance Processing: The Roles of Instance Count and Context Length (Chen ほか, 2026) | 16 モデル(GPT-5、Claude Sonnet 4.6、Gemini 3.1 Pro など) | Multi-instance 処理 8 タスク | Success rate | Spearman は instance 数 −0.61、総 context 長 −0.37 | 8 タスク×16 モデル×10 水準×5 seed | 200 instance 超で顕著、1,000 超で崩壊。既定の約 136 tokens/instance なら 200 instance は約 27K tokens(筆者換算) | 未記載 | [arXiv:2603.22608](https://arxiv.org/html/2603.22608) |
| Positional Failures in Long-Context LLMs: A Blind Spot in Reasoning Benchmarks (2026-05) | Qwen 2.5-7B、MiMo-v2-Flash、MiMo-V2.5-Pro、GLM-5.1、DeepSeek-V4-Pro、Kimi-K2.6 ほか | GSM8K、ARC-Challenge を filler 内に配置 | 中央と末尾の精度差(pp) | MiMo-v2-Flash の ws filler で −12(8K)、−24(32K)、−88(64K)。GLM-5.1 は 0、+2、+2 | 条件ごと N=50 | 脆弱なモデルでは 8K から | DeepSeek-V3.2(128K 窓)の最大帯は約 50% | [arXiv:2605.23170](https://arxiv.org/html/2605.23170v1) |
| When and How Context Rot Appears in Coding Agents: A White-Box Study of Agent Skills in Code Auditing (Xue, 2026-08) | Codex + gpt-5.4-mini(medium)。疎な probe は gpt-5.5、gpt-5.6-luna、deepseek-v4 系 | 実務由来の code audit、24 check | Strict success | Clean 8/10、long 3/10(Relevant、Irrelevant とも)。Fisher p=0.0698 | 条件ごと n=10 | **文字数**で記載。10,991 字は合格、90,537 字は不安定、299,140 字は 2 回とも失敗。著者は普遍的な onset を否定 | token 未記載のため算出不能 | [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2) |
| An Empirical Study of Harness Design for Coding Agents (Fan ほか, 2026-09) | Nemotron-3 30B/120B/550B、Mistral-Medium-3.5-128B | SWE-Bench Verified、Terminal-Bench 2.1 | Success rate | SWE の管理あり−なしの差は 32k で 35.7pp、128k で 2.7pp | 500 task + 89 task、176 設定 | 管理なしは溢れで失敗(T0 の溢れ率 32k で 78.7%、128k で 8.7%)。管理ありは主に 32k→64k でのみ上昇 | 予算は人工的な上限 | [arXiv:2609.20804](https://arxiv.org/html/2609.20804) |
| Beyond Resolution Rates: Behavioral Drivers of Coding Agent Success and Failure (Mehtiyev & Assunção, 2026-04) | 19 agent(8 framework、14 LLM) | SWE-bench Verified | 解決有無 | 課題固定では解決側が 44.0 step、失敗側が 39.6 step | 9,374 trajectory、500 task | **未記載**(step のみで token 分析なし) | 未記載 | [arXiv:2604.02547](https://arxiv.org/html/2604.02547v1) |
| ACON: Optimizing Context Compression for Long-horizon LLM Agents (2025-10) | gpt-4.1 ほか | AppWorld、OfficeBench、8-objective QA | Accuracy、peak token | AppWorld は圧縮なし 56.0%(peak 9.93K)、ACON 最良 56.5%(peak 7.33K) | AppWorld 168 task | **未記載**。長さを単独で操作した実験なし | 未記載 | [arXiv:2510.00615](https://arxiv.org/html/2510.00615) |
| Why Do Multi-Agent LLM Systems Fail? (MAST, 2025-03) | GPT-4、Claude 3、Qwen2.5、CodeLlama | Multi-agent 7 framework | 失敗モード頻度 | FM-1.4 Loss of conversation history は 2.80% | 1,642 trace | **未記載**。token や窓の分析なし | 未記載 | [arXiv:2503.13657](https://arxiv.org/html/2503.13657) |
| Claude Opus 4.6 system card | Opus 4.6 | MRCR v2 8-needle | Mean Match Ratio | 256K 帯 91.9、1M 帯 78.3(64k thinking)。max effort は 93.0、76.0 | 5 trial 平均 | (524k, 1024k] で −13.6pp | 12.8〜25.6% → 52.4〜102.4% | [Card p.30-32](https://www.anthropic.com/claude-opus-4-6-system-card) |
| Claude Sonnet 4.6 system card | Sonnet 4.6 | 同上 | 同上 | max effort で 90.3→65.8。64k で 90.6→65.1 | 5 trial 平均。<1M subset は max 29 問、64k 54 問 | (524k, 1024k] で約 −25pp | 同上 | [Card p.29-31](https://www.anthropic.com/claude-sonnet-4-6-system-card) |
| Claude Opus 4.7 system card | Opus 4.7(max) | MRCR v2、GraphWalks | Mean Match Ratio、F1 | MRCR 59.2→32.2。GraphWalks BFS は 256K subset 76.91、256K-1M 58.6 | GraphWalks は 5 trial | MRCR は 256K 帯ですでに 59.2 | 同上 | [Card p.194-196](https://www.anthropic.com/claude-opus-4-7-system-card) |
| GPT-5.4 発表 | GPT-5.4(1M 窓、標準 272K) | MRCR v2 8-needle | スコア | 帯順に 97.3、91.4、97.2、90.5、86.0、79.3、57.5、36.6 | 未記載 | 32K-64K 帯から緩やかに、256K 超で急に低下 | 256K-512K は 25.6〜51.2% | [OpenAI](https://openai.com/index/introducing-gpt-5-4/) |
| GPT-5.5 発表 | GPT-5.5(API 1M 窓、Codex 400K 窓) | MRCR v2 8-needle | スコア | 帯順に 98.1、93.0、96.5、90.0、83.1、87.5、81.5、74.0 | 未記載 | 非単調。明確な低下は 256K 超 | 512K-1M は 51.2〜100% | [OpenAI](https://openai.com/index/introducing-gpt-5-5/) |

Lost in the Middle の窓比の補足: GPT-3.5-Turbo(4K 窓)の 30 文書は平均 4,419 tokens で窓を超えるため、4K 版の窓比は表に載せていない。論文がこの設定を 4K 版でどう扱ったかは確認していない。token 数と窓は [arXiv:2307.03172](https://arxiv.org/abs/2307.03172) による。

OpenAI の 2 ページは openai.com への直接アクセスが 403 だったため、r.jina.ai の描画経由で読んだ。GPT-5.4 の 79.3 と 36.6 は [Opus 4.7 system card の Figure 8.7.2](https://www.anthropic.com/claude-opus-4-7-system-card) でも一致を確認した。

#### 補足の数値

- Chroma の Repeated Words では、幻覚的な語の混入が Gemini で 500-750 語から、GPT-4.1 の拒否が約 2500 語から始まると記載される。これは入力長ではなく出力長に依存する別タスクである — [Chroma](https://www.trychroma.com/research/context-rot)
- Chroma は NIAH の入力長を「8 input lengths」とのみ書き、具体的な token 値を本文に載せていない — [Chroma](https://www.trychroma.com/research/context-rot)
- Xue の toy migration task は 241,777 字でも合格した。長さだけでは context rot を説明できない例として著者が挙げている — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)
- 同研究の疎な probe で、gpt-5.5 は同じ主タスクで clean、relevant、irrelevant のいずれも 3/3 だった — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)
- Roig は「advertised context length is a poor proxy for usable context length」と述べる — [arXiv:2603.08274](https://arxiv.org/html/2603.08274)
- LongCodeBench の QA 帯は帯ごとにリポジトリと問題が異なる(帯別 27、16、20、15、11、9 repo)。単調性を強く読めない — [arXiv:2505.07897](https://arxiv.org/html/2505.07897)

### Inferences

- 2026 世代の MRCR v2 に限ると、GPT-5.4 の明瞭な低下点は 256K 付近で、GPT-5.5 と Opus 4.6 はより後ろに移っている。ただし Opus 4.7 は 256K 帯でもすでに低く、世代が新しいほど良いとは言えない。
- 劣化点を 1 つの token 数で語るのは誤りで、「モデル×タスク×内容」の条件付き分布として扱うべきだという Xue の結論は、横断比較の結果とも整合する。
- MRCR の帯ごとの差は小さな反転を含む(GPT-5.5 の 83.1→87.5、GPT-5.4 の 91.4→97.2)。数 pp の差で閾値を決めるのは過剰解釈である。

### Gaps

- 2026 世代の frontier モデルで、coding agent の成功率と実 context 使用率(%)を結んだ曲線は見つからなかった。Xue は文字数のみ、Mehtiyev & Assunção は step のみである。
- LongCodeBench は本文と Table 3 が一部食い違う(GPT-4.1 / GPT-4o の 11%、Gemini 2 Flash と GPT-4o のピーク値)。本ノートは Table 3 の値を使った。
- MRCR の帯ごとの問題数は確認できなかった。Sonnet 4.6 の <1M subset(29 問、54 問)から見て、帯によっては少数である可能性がある。
- 2026-11 時点の後続モデル(Claude Mythos Preview など)の system card は読んでいない。
- GPT-5.5 の deployment safety ページは読んだ範囲に MRCR の記載がなかった(末尾 15,627 字は未読) — [OpenAI](https://deploymentsafety.openai.com/gpt-5-5)

## 劣化は窓サイズに比例するのか、絶対トークン数に依存するのか

### Takeaway

どちらの単純モデルも成立しない。同一 checkpoint を 200K と 1M で比較した対照実験は見つからなかった。最も近い同系統比較では、拡張窓版(同一重みか不明)の性能は元の版とほぼ同じで(Lost in the Middle)、別 checkpoint の 1M 版(作成方法は未確認)は同じ 128k の standard で 3 倍以上高い(LongBioBench)。同じ 1M 窓・同じ帯でもモデルで大きく差が出るため、窓の公称値や比率だけでは説明できない。

### Cited Findings

- **拡張窓版との比較(Lost in the Middle Table 1、原文照合。同一重みかは論文に記載なし):** closed-book / oracle は Claude-1.3 が 48.3 / 76.1、Claude-1.3 (100K) が 48.2 / 76.4、GPT-3.5-Turbo が 56.1 / 88.3、GPT-3.5-Turbo (16K) が 56.0 / 88.6 だった。20 文書の位置別でも 75.8 / 75.7 のようにほぼ一致する。論文は拡張窓版が入力の利用に優れるとは限らないと述べる — [arXiv:2307.03172](https://arxiv.org/abs/2307.03172)
- **同系統で窓が違う比較(LongBioBench Fig. 9、ヒートマップ画像から読み取り):** 窓は Qwen2.5-7B-Instruct が 131,072、Qwen2.5-7B-Instruct-1M が 1,010,000(Table 6)。結果は下表の通り — [arXiv:2506.02921](https://arxiv.org/html/2506.02921v2)

| タスク | モデル | 2k | 8k | 16k | 32k | 64k | 128k |
|---|---|---|---|---|---|---|---|
| standard | 7B-Instruct (131K) | 99.8 | 82.9 | 72.2 | 71.9 | 56.9 | 26.1 |
| standard | 7B-Instruct-1M | 99.9 | 99.5 | 99.6 | 98.9 | 98.6 | 94.9 |
| calculation | 7B-Instruct (131K) | 100.0 | 74.6 | 76.2 | 75.1 | 50.7 | 21.6 |
| calculation | 7B-Instruct-1M | 100.0 | 99.9 | 99.1 | 98.2 | 96.4 | 90.2 |
| multihop_2 | 7B-Instruct (131K) | 95.4 | 37.9 | 39.0 | 24.0 | 9.8 | 1.2 |
| multihop_2 | 7B-Instruct-1M | 89.8 | 76.4 | 57.9 | 51.5 | 33.8 | 25.9 |
| twodiff | 7B-Instruct (131K) | 46.3 | 32.9 | 29.1 | 17.6 | 8.9 | 4.8 |
| twodiff | 7B-Instruct-1M | 51.5 | 36.9 | 21.1 | 14.6 | 6.7 | 3.7 |

- 同じ 1M 版の understanding は 256k で 48.0、512k で 0.3 に落ちるが、reasoning は 37.2、31.5 と崩れ方が違う。Qwen2.5-14B-1M も understanding が 47.0、2.7、reasoning が 40.3、28.8 である — [arXiv:2506.02921](https://arxiv.org/html/2506.02921v2)
- **harness 側の予算を変えた比較(Fan ほか):** 同一モデルで 32k、64k、96k、128k を比較した。管理なし(T0)は予算とともに上がる(Nemotron-3 550B は 6.40→29.40→51.00→59.80)。T1〜T4 の管理ありはモデル依存で、Mistral-Medium-3.5-128B はおおむね 63〜69 で横ばい、Nemotron-3 550B は 32k の 51〜58 から 96k 以降の 65〜67 に上がって頭打ちになる — [arXiv:2609.20804](https://arxiv.org/html/2609.20804)
- **同一モデルの帯比較(ただし窓は固定):** Opus 4.6 と Sonnet 4.6 の表はいずれも 1M 窓で評価した帯比較であり、窓の違いの比較ではない — [Opus 4.6 card](https://www.anthropic.com/claude-opus-4-6-system-card)、[Sonnet 4.6 card](https://www.anthropic.com/claude-sonnet-4-6-system-card)
- **ベンダーの顧客談:** Hex は Opus の窓を 200k から 500k に上げて総 token が減ったと述べ、Obvious は compaction イベントが 15% 減ったと述べる。いずれも対照実験ではない — [Anthropic 1M context GA](https://claude.com/blog/1m-context-ga)
- **token 量以外の要因:** Chen ほかは、instance 数を固定すると context 長と成功率の相関が弱く有意でなくなる(p>0.1)と報告し、「context length alone has limited explanatory power」と述べる — [arXiv:2603.22608](https://arxiv.org/html/2603.22608)

### Inferences

- **窓比の固定閾値は当てはまらない。** 同じ 1M 窓・同じ帯でも、MRCR v2 の 512K-1M は GPT-5.4 が 36.6、GPT-5.5 が 74.0、(524k, 1024k] は max effort で Opus 4.6 が 76.0、Sonnet 4.6 が 65.8 である。同じ窓比でも低下幅はモデルで大きく異なる。
- **ただし窓比の効果を否定もできない。** 窓の約 25% では、Qwen 131K 版の 32k(standard 71.9)、1M 版の 256k(understanding 48.0)、NoLiMa の GPT-4o の 32k(69.7)がいずれも低下している。指標とベンチマークが異なるため比較は緩いが、比率がまったく無関係だとは言えない。
- **絶対 token の固定閾値も当てはまらない。** 128k の standard は 7B-Instruct が 26.1、1M 版が 94.9 で、同じ絶対量でも 3 倍以上違う。ただし両者は別 checkpoint で、1M 版の 256k、512k の値は standard ではなく understanding の集計なので、同じ系列として延長できない。
- Lost in the Middle と LongBioBench で結果が逆に見えるのは、比較対象の違いで説明できる可能性がある。前者は約 6K tokens 以下の API モデルで、後者は別 checkpoint を 128k まで測っている。
- GPT-5.4 の明瞭な低下が標準窓 272K をまたぐ 256K-512K 帯で起きる点は、実用窓の設計と関係する可能性があるが、因果を示す根拠はない。
- 実務的には、compaction 閾値を窓比で固定する設計には根拠が薄い。モデルとタスクごとに実測するほうが裏付けがある。Fan ほかの soft 0.6、hard 0.85 は実験設定であり、最適値の結論ではない。

### Gaps

- 同一 checkpoint を 200K 窓と 1M 窓で走らせた比較は見つからなかった。Anthropic の system card はいずれも 1M 窓の評価で、OpenAI の Codex 400K と API 1M の比較も見つからなかった。
- Qwen2.5-7B-Instruct-1M の作成方法は、論文の Table 6 と読んだ範囲の本文では確認できなかった。
- Lost in the Middle の拡張窓版が元の版と同一重みかどうかは、論文からは分からない。
- Fan ほかの peak-context ratio は図のみで数値を確認していない。対象モデルも frontier モデルではない。

## 劣化は内容の種類(noise/distractor と関連情報)と情報の位置に依存するか

### Takeaway

内容の種類には強く依存する。特に needle と似た distractor は短い文脈でも効く。一方「関連情報なら長くても無害」とは言えず、coding agent の実験では関連・無関連ともに同程度に落ちた。位置効果は 2023 年の U 字が普遍ではなくなり、2026 世代では filler 次第で消えたり強く残ったりする。

### Cited Findings

#### 内容の種類

- GSM-IC は無関係な 1 文で、COT の base 95.0 に対し micro 72.4、macro 6.0(code-davinci-002)に下がる。著者は、Macro では base 問題のうち一貫して解ける割合が 30% 未満になったと述べる。instruction 付加で micro 77.8、macro 15.0 に改善する — [arXiv:2302.00093](https://arxiv.org/abs/2302.00093)
- Chroma は「Even a single distractor reduces performance」と述べ、needle-question 類似度が低いほど長さとともに速く低下すると報告する。数値は図のみで、2 つの topic では一般化できないとも認めている — [Chroma](https://www.trychroma.com/research/context-rot)
- Chroma では、文を shuffle して一貫性を壊した haystack のほうが性能が高かった — [Chroma](https://www.trychroma.com/research/context-rot)
- Chroma の LongMemEval では、平均約 300 tokens の focused prompt が平均約 113k tokens の full prompt を全系統で上回った。Claude の差が最大で、主因は回答の棄権である — [Chroma](https://www.trychroma.com/research/context-rot)
- NoLiMa では、質問語を含む distractor 文で GPT-4o の base が 93.8 に下がり、effective length が 1K に縮む。字句一致の選択肢は Llama 3.3 70B の 32K を 56.2 から 93.1 に引き上げる。つまり既存の long-context 評価は字句一致で過大評価されうる — [arXiv:2502.05167](https://arxiv.org/html/2502.05167v3)
- LongBioBench は distractor 密度と性能に「strong negative correlation」を報告する(密度の効果量は本文になし)。一貫した bio haystack への置換では、Qwen2.5-Instruct-7B-1M の性能差が −7.9%、−28.3%、−88.9%とタスクの難度に応じて拡大する — [arXiv:2506.02921](https://arxiv.org/html/2506.02921v2)
- Xue の code audit は、同じ文字数(299,140 字)の Relevant と Irrelevant がともに 3/10 だった(p=1.0)。coverage も 92.1%、93.8% と近く、rot は成果物全体ではなく少数の決定的な義務を落とす形で現れる。ただし CI は 39.7〜89.2% と広い — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)
- 同じ 30 万字でも、detailed check の注入で 10/10、generic check で 5/10 まで回復した(Generic と Detailed の p=0.0325、探索的) — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)
- LongCodeBench の Claude 3.5 Sonnet は、RAG で QA が 65.5〜71.7% から 12.77〜31.18% に悪化する一方、SWE では 32K の 29%→256K の 3% だったものが RAG で 16〜23% を維持する。絞り込みの効果はタスクで逆転する — [arXiv:2505.07897](https://arxiv.org/html/2505.07897)

#### 位置

- Lost in the Middle は U 字型の性能を報告した。GPT-3.5-Turbo の 20 文書は index 0 / 4 / 9 / 14 / 19 で 75.8 / 57.2 / 53.8 / 55.4 / 63.2 だった。中央に置くと、文書なしの closed-book(56.1)を下回る場合がある — [arXiv:2307.03172](https://arxiv.org/abs/2307.03172)
- Chroma は 11 位置を試し、この NIAH タスクでは「no notable variation」と結論した — [Chroma](https://www.trychroma.com/research/context-rot)
- NoLiMa は 32K で中央の dip を観察しつつ、末尾 2K の分析から「the main limiting factor is the increased context length」と結論した — [arXiv:2502.05167](https://arxiv.org/html/2502.05167v3)
- Positional Failures の中央−末尾差(GSM8K、8K / 32K / 64K)は、ws filler で MiMo-V2.5-Pro が −8 / −16 / −32、GLM-5.1 が 0 / +2 / +2、DeepSeek-V4-Pro が −2 / −2 / −4、Kimi-K2.6 が +2 / −2 / +2 だった — [arXiv:2605.23170](https://arxiv.org/html/2605.23170v1)
- 同じモデルでも qo_v2 filler では、DeepSeek-V4-Pro が −32 / −16 / −18、GLM-5.1 が −36 / −40 / −50、MiMo-V2.5-Pro が −56 / −44 / −54、Kimi-K2.6 が −20 / −38 / −34 と大きく出る — [arXiv:2605.23170](https://arxiv.org/html/2605.23170v1)
- neutral filler では、Qwen を除く初期モデルは 1 点を除き ±4pp に収まる — [arXiv:2605.23170](https://arxiv.org/html/2605.23170v1)
- 著者は N=50 で 95% CI が p=0.5 で約 ±14pp、64K の多くは単一 seed だと注記している — [arXiv:2605.23170](https://arxiv.org/html/2605.23170v1)
- LongBioBench は lost-in-the-middle を観察したが、簡単なタスクでは弱く、密度・位置の両効果は reasoning でより強いと述べる — [arXiv:2506.02921](https://arxiv.org/html/2506.02921v2)
- Xue は GPT-5.5 の単発 probe で、重要な指示が中央でも末尾でも合格したと報告する。位置効果の検証としては不十分である — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)

### Inferences

- 「長さ」と「内容」は独立ではない。needle や target に構造が似た filler(MRCR の同種要求、qo_v2 の質問列、一貫した haystack)で劣化が強く、neutral な filler では弱い。
- coding agent の context は、ファイル、ログ、過去の指示という互いに類似した内容の集積であり、neutral text より distractor 寄りの条件に近いと考えられる。ただしこれを直接測った研究はない。
- Relevant と Irrelevant が同等という Xue の結果は「内容は無関係」を意味しない。n=10 で CI が広く、Irrelevant の除外が多いため、同等性を示せていない。
- Xue の detailed check の結果と LongCodeBench の RAG の逆転から、context を減らすよりも、決定的な義務を末尾や明示 check として再提示するほうが効く場面があると考えられる。ただし前者は探索的である。

### Gaps

- 2026 世代の frontier モデル(Opus 4.6 以降、GPT-5.5)に対する、token 数を揃えた位置効果の統制実験は見つからなかった。
- 実際の coding agent の trace における distractor 密度と失敗の関係を測った研究は見つからなかった。
- Chroma の distractor と LongMemEval の差は本文に数値がなく、図から値を読み取っていない。

## どの結果が矛盾し、どの結果が 2025-2026 世代や長時間 coding agent に当てはまらないか

### Takeaway

主要な矛盾は、位置効果、関連情報の扱い、trajectory 長、ベンダー主張と自社データの 4 点である。多くは条件の違いで説明できる。2023〜2025 年前半の絶対値は現行モデルに流用できず、長時間 agent に直接適用できる統制研究は Xue と Fan ほかにほぼ限られ、どちらも強い制約がある。

### Cited Findings

#### 矛盾

- **ベンダー主張と自社 system card:** 1M GA の blog は「Claude Opus 4.6 and Sonnet 4.6 maintain accuracy across the full 1M window」と書く — [Anthropic](https://claude.com/blog/1m-context-ga)。しかし Sonnet 4.6 の card は MRCR v2 で 90.3→65.8(max)を示す — [Sonnet 4.6 card](https://www.anthropic.com/claude-sonnet-4-6-system-card)。ただし全 1M 帯は窓を超える問題を含み、<1M subset は 77.8(max、29 問)、71.3(64k、54 問)である — [Sonnet 4.6 card](https://www.anthropic.com/claude-sonnet-4-6-system-card)
- **Opus 4.6 の 76% と 78.3%:** 二次情報で不一致に見えるが、原文では 76.0 が max effort、78.3 が 64k extended thinking である。矛盾ではない — [Opus 4.6 card](https://www.anthropic.com/claude-opus-4-6-system-card)、[Anthropic 発表](https://www.anthropic.com/news/claude-opus-4-6)
- **世代と性能:** Opus 4.7(max)の MRCR は 59.2 / 32.2 で、Opus 4.6(64k thinking)の 91.9 / 78.3 を下回る。一方 GraphWalks の 256K subset は Opus 4.7 が BFS 76.91、Parents 93.57、Opus 4.6 が BFS 61.5、Parents 95.1 で、BFS では逆転する。effort 設定が揃っていない点に注意が必要である — [Opus 4.7 card](https://www.anthropic.com/claude-opus-4-7-system-card)、[Opus 4.6 card](https://www.anthropic.com/claude-opus-4-6-system-card)
- **GraphWalks の GPT-5.4 値:** GPT-5.4 の発表は BFS 256K-1M を 21.4、GPT-5.5 の発表は BFS 256K を 62.5、1M を 9.4 とする。帯の定義が異なり、そのまま比較できない — [GPT-5.4](https://openai.com/index/introducing-gpt-5-4/)、[GPT-5.5](https://openai.com/index/introducing-gpt-5-5/)
- **非単調な値:** Sonnet 4.5 は 256K で 10.8、1M で 18.5 と長いほうが高い。原因は card に記載がない — [Opus 4.6 card](https://www.anthropic.com/claude-opus-4-6-system-card)
- **位置効果:** Lost in the Middle の U 字に対し、Chroma は差なし、NoLiMa は長さが主因とする。Positional Failures は同一モデルでも filler で 0pp から −50pp まで変わる — [LitM](https://arxiv.org/abs/2307.03172)、[Chroma](https://www.trychroma.com/research/context-rot)、[NoLiMa](https://arxiv.org/html/2502.05167v3)、[Positional Failures](https://arxiv.org/html/2605.23170v1)
- **trajectory 長:** Understanding Code Agent Behaviour: An Empirical Study of Success and Failure Trajectories(Majgaonkar ほか)は失敗 trajectory のほうが長いと報告する — [arXiv:2511.00197](https://arxiv.org/abs/2511.00197)。Mehtiyev & Assunção も agent 固定では同じ結果(+14.0〜+111.9%)を得るが、課題固定では逆転する(解決 44.0 step、失敗 39.6 step、416 課題中 263 で解決側が長い、p=1.9×10⁻⁹) — [arXiv:2604.02547](https://arxiv.org/html/2604.02547v1)
- **長さの主因性:** Chroma は「model performance consistently degrades with increasing input length」と述べる — [Chroma](https://www.trychroma.com/research/context-rot)。Chen ほかは instance 数を固定すると相関が消えるとする — [arXiv:2603.22608](https://arxiv.org/html/2603.22608)。LongCodeBench の QA は 1M でも GPT-4.1 が 80.0 で 32K の 72.6 を上回る — [arXiv:2505.07897](https://arxiv.org/html/2505.07897)
- **同一モデルでもタスクで逆:** LongCodeBench の Claude 3.5 Sonnet は QA がほぼ平坦(65.5〜71.7)で、SWE は 29→3 に落ちる — [arXiv:2505.07897](https://arxiv.org/html/2505.07897)
- **compaction の価値:** Anthropic は「lossy summarization」などが不要になったと書く — [Anthropic](https://claude.com/blog/1m-context-ga)。Fan ほかでは 128k で管理の利得は小さいが(2.7pp)、32k では 35.7pp と大きい — [arXiv:2609.20804](https://arxiv.org/html/2609.20804)。ACON では AppWorld の圧縮なし 56.0、最良 56.5 で、精度はほぼ同じまま peak が減る — [arXiv:2510.00615](https://arxiv.org/html/2510.00615)

#### 現行モデルや長時間 agent への適用限界

- GSM-IC の対象は code-davinci-002、text-davinci-003 である — [arXiv:2302.00093](https://arxiv.org/abs/2302.00093)
- Lost in the Middle の対象は Claude-1.3、GPT-3.5 世代で、文脈は最大数千 tokens である — [arXiv:2307.03172](https://arxiv.org/abs/2307.03172)
- NoLiMa と LongCodeBench の主表は GPT-4o、Claude 3.5 Sonnet などを含む旧世代中心である — [NoLiMa](https://arxiv.org/html/2502.05167v3)、[LongCodeBench](https://arxiv.org/html/2505.07897)
- Chroma は Claude Opus 4、o3、Gemini 2.5 までで、検索タスク中心、査読なしの技術レポートである — [Chroma](https://www.trychroma.com/research/context-rot)
- 2026 世代の改善例として、MiMo-v2-Flash の −88pp(64K、ws)は MiMo-V2.5-Pro で −32pp に縮む — [arXiv:2605.23170](https://arxiv.org/html/2605.23170v1)。GPT-5.4 の 512K-1M は 36.6 だが GPT-5.5 は 74.0 である — [OpenAI](https://openai.com/index/introducing-gpt-5-5/)
- MAST は trace の平均が 15,000 行超と述べるが token 分析はない。context size の根拠としては使えず、FM-1.4(2.80%)は失敗モードの頻度にすぎない — [arXiv:2503.13657](https://arxiv.org/html/2503.13657)
- ACON の peak は約 7〜12K tokens であり、1M 級の話ではない。難度別の低下(Easy 80.7%、peak 7.57K / Hard 39.7%、peak 11.95K)は難度と交絡している — [arXiv:2510.00615](https://arxiv.org/html/2510.00615)
- Xue は 1 タスク、主に 1 モデルで、n=10、pilot 後のサンプル拡張があり、extension attempt のうち 28 件(うち 20 件が Irrelevant)を除外した。Clean と long の差は p=0.0698 で 5% 水準に届かない — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)
- 同研究の 2.4M 文字と 3 回の native compaction の worker は単一 run で、著者は閾値推定ではないと明言する — [arXiv:2607.17937](https://arxiv.org/html/2607.17937v2)
- Fan ほかは open-weight モデルを人工的な予算で制限した実験で、Terminal-Bench の対比の多くは有意でないとされ、1M 級は扱っていない — [arXiv:2609.20804](https://arxiv.org/html/2609.20804)
- Mehtiyev & Assunção は agent ごと課題 1 run で、run 間のばらつきを測っておらず、token 分析もない — [arXiv:2604.02547](https://arxiv.org/html/2604.02547v1)
- MRCR、GraphWalks はいずれも検索・グラフ処理の合成タスクであり、1 回の長い prompt で測る。tool call を重ねる agent の状態追跡とは別物である。MRCR の定義は [Opus 4.6 card](https://www.anthropic.com/claude-opus-4-6-system-card) を参照。

### Inferences

- 矛盾の多くは「何を測るか」の差であり、単純検索では長さへの耐性が高く、複数回の参照、順序、合成を要するタスクでは低下が早い。LongCodeBench の QA と SWE の差、LongBioBench の understanding と reasoning の差も同じ方向を示す。
- 旧世代の絶対値(GPT-4o の 32K で 69.7 など)は、現行モデルの閾値として流用すべきでない。一方「distractor に弱い」「filler 次第で位置に弱い」という定性的な傾向は、2026 世代でも一部残っている。
- coding agent への当てはめで最も強く言えるのは次の 3 点にとどまる。
  1. 同じ長さでも task と model で結果が変わる。
  2. 失敗 trajectory が長いという相関は難度の交絡を含む。
  3. 管理の利得は窓が狭いほど大きい。
- 「Opus 4.6 は 1M まで持つ」「Opus 4.7 は長文脈に弱い」といった結論を MRCR だけから agent 性能へ外挿するのは避けるべきである。

### Gaps

#### 確認できなかった主張

- **Boris Cherny の「MRCR を段階的に廃止」発言:** 原文の X post は確認できず、[Reddit のミラー](https://reddit.sentinel-team.org/posts/1sn57af/snapshots/2026-04-18T01%3A10%3A06.838236Z)や [Telegram の転載](https://t.me/s/mirra_bcherny)という二次情報のみである。Opus 4.7 の card 本文にこの説明は見当たらなかった。
- **Morphllm の「1M 窓モデルが 50K で低下」:** 検索 snippet の主張で、Chroma の原文では確認できない。採用しない — [Morphllm](https://www.morphllm.com/context-rot)
- **二次情報の「Opus 256K→1M で約 14% 低下」:** 原文では 91.9→78.3 の 13.6pp、または 93.0→76.0 の 17.0pp である。「%」表現は不正確なので採用しない — [chaseai.io](https://chaseai.io/blog/claude-1m-context-window-context-rot)
- **RULER の詳細:** abstract のみ確認した。「4 モデル(GPT-4、Command-R、Yi-34B、Mixtral)のみ」は検索 snippet 由来で本文未照合である — [arXiv:2404.06654](https://arxiv.org/abs/2404.06654)
- **BABILong の counting 28%:** NoLiMa 経由の snippet のみで、BABILong 本体は未確認である。
- **abstract のみ確認:** NeedleBench: Evaluating LLM Retrieval and Reasoning Across Varying Information Densities [arXiv:2407.11963](https://arxiv.org/abs/2407.11963)、Michelangelo: Long Context Evaluations Beyond Haystacks via Latent Structure Queries [arXiv:2409.12640](https://arxiv.org/abs/2409.12640)、[arXiv:2511.00197](https://arxiv.org/abs/2511.00197) は本文の数値を確認していない。
- **Gemini 系の MRCR 帯別スコア:** Gemini 3.1 Pro の 59.1、25.9 と Gemini 3 の値は Anthropic card 経由(Context Arena 由来)でのみ確認した。Google の一次資料は未読である。
- **存在しない arXiv ID:** 今回 fetch した ID はすべて実在した。存在が疑われる論文は見つからなかった。

#### 手法上の未解決点

- OpenAI の数値は r.jina.ai の描画経由であり、openai.com からの直接取得はできなかった(GPT-5.4 の 79.3、36.6 のみ Anthropic card で裏付けた)。
- 原文照合したのは、system card 3 件、Lost in the Middle、GSM-IC、LongBioBench Fig. 9、および arXiv と Chroma の主要数値の抜き取りである。それ以外の表内数値は要約経由で、誤読の可能性が残る。
- 共有 scratchpad の上書きの連絡を受け、上記の原文照合は一次ソースを専用の新規ディレクトリに再取得して再実施し、照合対象の文字列はすべて一致した。`scratchpad/papers` の抽出ファイルは使っていない。Fig. 9 は再取得した画像と初回の画像がバイト単位で一致した。
- 窓比はすべて筆者の計算で、ソース間の tokenizer 差を補正していない。
- 2607.17937、2603.22608、2510.00615、2503.13657、2609.20804 の WebFetch は本文の冒頭 100,000 字までを対象とした。抜き取り照合は全文に対して行ったが、後半の表の網羅的な読解はしていない。
