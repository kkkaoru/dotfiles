# summarization / compaction に高い reasoning effort を使う価値はあるか(横断比較)

確認方法: 数値は原則として arXiv HTML 本文(marina は GitHub API の PR 本文)で確認した。例外は各所に **[抽出のみ]** と記す。これは WebFetch の小型モデルによる抽出のみの値を指す。**[行未照合]** は、数値の存在は本文で確認したが、表の行との対応は未確認の値を指す。調査日は 2026-11-05。

## Q1. reasoning を増やすと summary 品質、または compaction 後の agent の downstream 成功率は上がるか。直接証拠はあるか

### Takeaway
**summarizer の reasoning effort だけを変えて compaction 後の downstream 成功率を測った研究は見つからなかった。** 最も近い証拠は、単一モデル内で effort を振った 2512.03503 の Fig. 6 のみである。effort を上げると faithfulness は下がり、reference-based 品質は横ばいだった(downstream は未測定で、effort 別の数値は図のみ)。

### Cited Findings
**直接に近い証拠(同一モデルで effort を変化)**
- GPT-5 の think ability を minimal から high まで変えると「factual faithfulness consistently declines」。AlignScore の低下が最大で、SummaC は同傾向でより緩やか、ROUGE / BERTScore は横ばい。著者は「undermines factual grounding without improving summary quality」と結論している — [Understanding LLM Reasoning for Abstractive Summarization, Sec. 5.3 / Fig. 6](https://arxiv.org/html/2512.03503v1)

**モデル間比較(effort の比較ではない)**
- 8 データセット平均(各 100 件、計 800 pair)の AlignScore は GPT-4.1 Vanilla 72.18、o1 65.70、o3 69.63、GPT-5 72.86(LRM は think ability = medium 固定)。ROUGE は Vanilla 19.24、GPT-5 16.99。[行未照合] — [同論文 Tables 3, 5](https://arxiv.org/html/2512.03503v1)
- 人手評価(1–5 点、博士学生 2 名)の faithfulness は GPT-5 4.42、SC 4.27、Vanilla 4.22、Deco 4.22、E2A 3.98。評価サマリ数は明記なし。[行未照合] — [同論文 Table 7](https://arxiv.org/html/2512.03503v1)
- dialogue summarization の SAMSum ROUGE-1 は o1 45.06 vs GPT-4o 47.01、DeepSeek-R1 43.90 vs V3 47.21、QwQ-32B 43.58 vs Qwen2.5-32B 44.88 で、reasoning 側がいずれも低い。別モデル同士の比較である。[行未照合] — [Reasoning or Not?, Table 2](https://arxiv.org/html/2507.02145v1)

**compaction 文脈の近接証拠(いずれも effort の効果とは読めない)**
- rule 喪失は compaction あたり Opus 4.6(effort max)0.51/3、GPT-5.4(medium)1.76、GPT-5.3-Codex(medium)2.73。effort はモデル・提供元と完全に交絡している — [Benchmarks Are Surprisingly Compaction-Friendly, Table 1 と Sec. 4.2](https://arxiv.org/html/2610.31778)
- 「reasoning block 削除」(GPT-5.4 で PaperBench 0.59→0.43、Opus 4.6 と GPT-5.3-Codex は影響なし)は summarizer の**入力**を変える操作で、effort の操作ではない — [同論文 Sec. 3.3, 4.1](https://arxiv.org/html/2610.31778)
- Bud ContextCompact 2.0 では summariser が agent の reasoning 設定を継承する。Qwen3.8-27B(thinking 有効)、100 タスク / 556 graded sessions で、長いセッションの解決率が 41%→53%(+12.3 pp、95% CI +1.6〜+23.0、p=0.043)。比較は compaction の有無で、effort は変えていない — [Bud ContextCompact 2.0](https://arxiv.org/html/2610.25516v1)
- marina PR #529 では、summary 呼び出しの reasoning を切ると fact retention が Haiku 5.5 +0.29 [0.21, 0.37]、DeepSeek v4.1 Flash +0.42 [0.33, 0.49]、Qwen 3.5 Flash +0.27 [0.20, 0.34] 上がった。原因は、500 token 上限を reasoning が使い切り、Haiku で 95%、DeepSeek で 82% の compaction が rule-based summary に fallback したこと。つまり出力枠の枯渇であり、reasoning が内容を損なった証拠ではない。査読なしの engineering pilot(40 transcripts)で、指標も fact retention である — [h2oai/marina PR #529](https://github.com/h2oai/marina/pull/529)

**reasoning 一般の間接証拠(summarization の研究ではない)**
- o1-like モデルは、簡単な質問の比較(Fig. 1(a))で conventional モデルより平均 1,953% 多く token を使う。数学ベンチマークでは 92% 超のケースで最初の solution ラウンドが既に正解だった。ベンチマーク全体や summary には一般化できない — [Do NOT Think That Much for 2+3=?](https://arxiv.org/html/2412.21187v2)
- 制約追跡タスク(BBEH Zebra Puzzles 200 問)では、natural overthinking の方が controlled より inverse scaling が強い(Takeaway 5、定性)。別タスクの code distractor 付き counting では Opus 4 が near-perfect から約 80% に低下し、five distractor の natural 設定では DeepSeek R1 が 70%→30% に低下した — [Inverse Scaling in Test-Time Compute, Sec. 4](https://arxiv.org/html/2507.14417v2)

### Inferences
- 「高 effort は summary に効く」という仮説を支持する証拠はなく、同一モデル内の唯一の測定は逆方向を示す。ただし downstream に効かないことも証明されておらず、正確には**未検証**である。
- モデル間では GPT-5 の AlignScore は Vanilla と同等だが、GPT-5 内で effort を上げると faithfulness は下がる。「reasoning モデルであること」と「effort を上げること」は別の変数として扱うべきである。
- 2507.02145 の ROUGE 差は reference 一致度の差で、faithfulness や downstream の劣化を直接示さない。

### Gaps
- Fig. 6 の effort 別の数値は本文・表になく、Fig. 6 専用の n も明記がない。
- 必要な実験は「agent と summarizer model を固定し effort のみ変更、同一 transcript で downstream 成功率と constraint 違反を測定」だが、発見できなかった。検索は数系統のみで、網羅性は保証しない。
- 2610.31778 で summary 呼び出しが agent と同じ effort を継承するかは確認できなかった(self-summarization であることのみ確認)。

## Q2. model strength、compression guideline、summary length はレバーとしてどう比較できるか

### Takeaway
効果量は、(1) constraint を summary の外に構造的に分離、(2) summarizer model の選択と極端な圧縮の回避、(3) guideline の順に見える。指標と条件が研究ごとに異なるため、序列は目安である。effort をこれらと並べて測った研究はない。

### 横断比較表
| 研究 | モデル | タスク | 指標 | 操作変数と効果量 | n |
|---|---|---|---|---|---|
| [ACON 2510.00615](https://arxiv.org/html/2510.00615) Table 1 | gpt-4.1 agent / compressor | AppWorld | 平均精度、peak tokens | guideline: Prompting 43.5→ACON 51.2 / 56.5、No compression 56.0。peak 9.93K→7.33K | 168 タスク |
| ACON Table 3 | optimizer: o3 / gpt-5 / gpt-4.1 | AppWorld | 平均精度 | 最適化 model: 51.2 / 50.6 / 47.6 | 表に明記なし |
| [Lost in Context Compaction 2608.11242v2](https://arxiv.org/html/2608.11242v2) Table 10 | gpt-oss-120b、Qwen3-30B-A3B | WildChat | SC retention、compliance | SC 保持の指示を追加: retention 1.3→24.8%、3.3→37.6%。compliance 54.6→65.5%、56.2→71.2% | 750 / 条件 |
| 同 Table 8 | GPT-5.4-mini | Hermes / OpenResearcher / WildChat | SC retention | Anthropic→Pi prompt: 53.7→88.4、26.0→98.0、28.3→6.7 | 300(Hermes 285)、220K context |
| 同 Table 4 | Qwen3.5-9B extractor | 3 シナリオ | SC retention | 分離抽出: 90.3 / 93.7 / 95.1%(平均 93.0%) | 未抽出 |
| [Governance Decay 2606.22528v2](https://arxiv.org/html/2606.22528v2) | 7 モデル | ConstraintRot 9 タスク | 違反率 | compaction: 0→30%(最大 59%) | 1,323(27 / cell) |
| 同 Fig. 3 | pooled | 同上 | 生存率、違反率 | budget 300→15 語: 生存 88→23%、違反 7→28% | 未抽出 |
| 同 Table 3 | DeepSeek、GLM、Claude | soft 5 タスク | 違反率 | summarizer: GLM 7–13%、Claude 47–60%、DeepSeek 33–93% | **15 / cell** |
| 同 pinning | 7 モデル | 同上 | 違反率 | 約 47 token の pinned policy で 0% | 未抽出 |
| [Agents Forget the Rules 2609.11024](https://arxiv.org/html/2609.11024v1) | 5 モデル | Forge-Bench | Loss of Control | constraint-faithful compaction 0%、constraint 省略 87% | 300 観測 |
| [Compaction-Friendly 2610.31778](https://arxiv.org/html/2610.31778) | Opus 4.6、GPT-5.4、GPT-5.3-Codex | PaperBench | 精度 | Haiku 4.5 で要約: GPT-5.4 0.59→0.43、Codex 0.38→0.18、Opus は CI 内 | **3 論文、2 run / 設定** |
| 同 | 同上 | ProgramBench | rule 喪失 / 3 | モデル別 0.51 / 1.76 / 2.73。回数別 0.92(初回)→2.65(30 回目) | 2 タスク、2 run / 設定 |
| [CompactionRL 2607.05378](https://arxiv.org/html/2607.05378v1) | agent 固定 GLM-4.7-Flash | SWE-bench Verified | pass@1 | summarizer: Qwen3.5-27B 55.5、GLM 50.5、Qwen3-30B-A3B 49.0 | 未抽出 |
| [RLM 2512.24601](https://arxiv.org/html/2512.24601) | GPT-5 vs GPT-5-nano | BrowseComp-Plus | 性能(定性) | summarizer の差は「comparable」 | **20** |
| [marina PR #529](https://github.com/h2oai/marina/pull/529) | DeepSeek v4.1 Flash | 公開ベンチの transcript | fact retention | 上限 500 / 800 / 1,200: 0.70 / 0.73 / 0.75(有意差なし) | **40**、査読なし |
| [Reasoning or Not? 2507.02145](https://arxiv.org/html/2507.02145v1) | 3 model pair | SAMSum 他 | ROUGE 他 | Q1 参照 | 819 / 1,500 / 800 / 279 |
| [2512.03503](https://arxiv.org/html/2512.03503v1) | GPT-4.1、o1、o3、GPT-5 | 8 データセット | AlignScore 他 | Q1 参照 | 100 / データセット |
| [Goal Drift 2505.02709](https://arxiv.org/html/2505.02709v1) | Claude 3.5 Sonnet / Haiku、GPT-4o / mini | 株取引シミュレーション | GD スコア | Q3 参照(compaction の研究ではない) | 20 seeds |

### Cited Findings
**構造的分離**
- Constraint Pinning は約 47 tokens で違反率を 0% に戻す — [Governance Decay](https://arxiv.org/html/2606.22528v2)
- extractor は retention 平均 93.0% で、WildChat では targeted prompt の最高値 37.6% を 52.7 pp 上回る — [Lost in Context Compaction v2](https://arxiv.org/html/2608.11242v2)
- pinning にも限界があり、operator を騙る rescind 通知で naive pinning の違反率は 0→17%、provenance label 付きで 10%。[抽出のみ] — [Governance Decay](https://arxiv.org/html/2606.22528v2)

**model strength**
- 「violation tracks the summarizer (rows), not the agent (columns)」。GLM の summary は 7–13%、DeepSeek と Claude の summary は 33–93%。GLM-5.1 が agent でも DeepSeek の summary を与えると 53% に上がる — [Governance Decay, Table 3](https://arxiv.org/html/2606.22528v2)
- 小さい summarizer の影響はモデル依存で、Opus 4.6 は Haiku 4.5 要約でも CI 内、OpenAI 2 モデルは低下した。著者は同一 provider が理由の可能性を挙げている — [Compaction-Friendly, Sec. 4.1](https://arxiv.org/html/2610.31778)
- GPT-5-nano と GPT-5 の summary は BrowseComp-Plus で comparable だが、20 サンプルでの確認である — [Recursive Language Models](https://arxiv.org/html/2512.24601)
- 「強さ」は単純な序列にならない。Governance Decay では GLM-5.1 の summary が Claude-Sonnet-4.6 より低リスクで、CompactionRL では 27B の Qwen3.5 が 30B の Qwen3 を上回る — [Governance Decay](https://arxiv.org/html/2606.22528v2)、[CompactionRL](https://arxiv.org/html/2607.05378v1)

**guideline**
- ACON は failure analysis で guideline を改善し、peak tokens を 26–54% 削減、小型 LM で 20–46% の性能改善を得た。8-objective QA では no compression を EM / F1 で上回りつつ peak tokens を 54.5% 削減した — [ACON](https://arxiv.org/html/2510.00615)
- 2610.31778 の summary prompt は「constraints … that should persist」を明示的に求めていたが、rule は落ちた — [Compaction-Friendly, Sec. 4.2](https://arxiv.org/html/2610.31778)
- SC 保持指示を足しても retention は 40% 未満に留まる — [Lost in Context Compaction v2, Table 10](https://arxiv.org/html/2608.11242v2)
- prompt の効果はデータセットで逆転し、GPT-5.4-mini の Pi prompt は OpenResearcher 98.0 だが WildChat 6.7(Anthropic prompt は 28.3)— [同 Table 8](https://arxiv.org/html/2608.11242v2)
- 「more capable」なモデルほど goal elicitation 強度の影響が大きい(Finding 3)。これは system prompt の話で、summary の guideline ではない — [Evaluating Goal Drift](https://arxiv.org/html/2505.02709v1)

**length**
- budget 300→15 語で生存率 88→23%、DeepSeek の違反率は 20→65% — [Governance Decay, Fig. 3](https://arxiv.org/html/2606.22528v2)
- 上限 500 / 800 / 1,200 tokens で 0.70 / 0.73 / 0.75(有意差なし)。ただし 500 では DeepSeek の summary の 93%、800 では 10% が途中で切れた — [marina PR #529](https://github.com/h2oai/marina/pull/529)
- 100K tokens 入力での圧縮比は平均 182× — [Lost in Context Compaction v2](https://arxiv.org/html/2608.11242v2)

### Inferences
- 喪失の大半は「モデルが考え足りない」より「summary という経路に constraint が載らない」問題に見える。その場合、effort の増加より構造的分離の方が筋がよい。
- length は閾値型のレバーらしく、極端に短いと悪化し、十分なら差が小さい。指標の異なる 2 研究からの推測である。
- 表の効果量は単位が異なるため、同じ尺度では比較できない。序列を信頼できるのは同一研究内の比較に限られる。

### Gaps
- 3 レバーと effort を同一実験内で交差させた研究は見つからなかった。
- **研究間の対立(未解決)**: [2603.17781](https://arxiv.org/html/2603.17781) は compaction loss を「architectural, not model-specific」と述べるが、2606.22528 と 2610.31778 は強いモデル依存を示す。
- ACON の小型 LM 20–46% のモデル別内訳は抽出していない。

## Q3. summary で何が失われるか。reasoning はそれを減らすか

### Takeaway
最もよく測られている損失は hallucination ではなく **constraint の脱落(omission)** である。goal は残り、作業の進め方に関する制約が落ちる。reasoning がこれを減らすという証拠は見つからず、faithfulness については effort とともに悪化する報告が 1 件ある。

### Cited Findings
**constraint 脱落**
- 実セッション(SWE-chat 44 セッション、269 constraints)で 81% 保持、19% 脱落(別 judge では 24.5%)。脱落率は feature requirements 6.2%、process instructions 35.9%、scope exclusions 36.8%。[数値は本文、カテゴリ対応は抽出のみ] — [Compaction-Friendly](https://arxiv.org/html/2610.31778)
- summary は「やること」を残し「やり方」の制約を落としやすく、benchmark score はそれを測らない。スコアが不変でも rule は失われる — [同論文](https://arxiv.org/html/2610.31778)
- compactor は注入 SC の平均 17% しか保持しない。inbox 実験(gpt-oss-120b、20 seeds × 5 message)の無断削除は compaction 前 0 に対し、後は 30/100 セッション — [Lost in Context Compaction v2](https://arxiv.org/html/2608.11242v2)
- 保持されれば違反率 0%(n=90)、脱落すると 38%(n=315)。soft policy は +50 pts、hard norm は +6 pts(8.3× の差)— [Governance Decay](https://arxiv.org/html/2606.22528v2)
- 制約劣化かつ機会ありの LoC は 55.00%、intact constraints では 9.33% — [Agents Forget the Rules](https://arxiv.org/html/2609.11024v1)

**累積**
- rule 喪失は 0.92(初回)、1.93(10 回目)、2.41(15 回目)、2.65(30 回目)と増える。後の summary は既に rule を失った summary から書かれる — [Compaction-Friendly](https://arxiv.org/html/2610.31778)
- 20 constraints・88 ターンのシミュレーションで、1 / 2 / 3 ラウンド後の保持は 91% / 62% / 46%(5 seed 平均)。2,000 facts の 36.7× 圧縮で fact の 60% を喪失 — [Facts as First-Class Objects](https://arxiv.org/html/2603.17781)

**hallucination / faithfulness**
- 2603.17781 は「The model does not hallucinate」「Compaction is lossy, not noisy」と記述している — [Facts as First-Class Objects](https://arxiv.org/html/2603.17781)
- explicit reasoning の SC / IR は AlignScore を 60.03 / 60.13 に下げる(Vanilla 72.18)。BERTScore vs AlignScore は r = −0.685、p = 0.014 — [Understanding LLM Reasoning](https://arxiv.org/html/2512.03503v1)
- LLM judge は faithfulness を過大評価する傾向があり、G-Eval は 4.96–4.99 に張り付く(人手は 3.98–4.42)— [同論文](https://arxiv.org/html/2512.03503v1)
- 2507.02145 に専用の faithfulness 指標はなく、LLM judge の Consistency のみ。DeepSeek-R1 の hallucination 事例は判定に疑問の余地がある(WebFetch 抽出時の指摘で、原文は未検証)。[抽出のみ] — [Reasoning or Not?](https://arxiv.org/html/2507.02145v1)

**goal drift**
- system goal が profit の場合、weak elicitation の GDactions は 0.51–0.93、emission では 0.25–0.37。scaffolded Claude 3.5 Sonnet は 100,000 tokens 超でほぼ完全に goal を維持し、drift は pattern-matching と相関する。長い context と競合目標の研究で、compaction の研究ではない — [Evaluating Goal Drift](https://arxiv.org/html/2505.02709v1)

**失敗の基準率(MAST)**
- context loss(FM-1.4)2.80%、task 仕様不遵守(FM-1.1)11.8%、task derailment(FM-2.3)7.40%。summary も reasoning も操作していない。MetaGPT では GPT-4o が Claude 3.7 Sonnet より FC1 失敗が 39% 少ない — [Why Do Multi-Agent LLM Systems Fail?](https://arxiv.org/html/2503.13657)

### Inferences
- 2 回目以降の summary が前回の summary から書かれるなら、失われた rule は reasoning を増やしても復元できない(transcript を再読しない前提)。effort が効く余地があるのは、原文を見られる初回の重要度判断である。
- Opus 4.6(max)の低い喪失率は effort の効果ともモデルの特性とも読めるので、高 effort の根拠にはならない。
- faithfulness を LLM judge だけで評価すると、effort の害を見逃す可能性がある。

### Gaps
- 実 compaction summary の hallucination 率を測った研究は見つからなかった。文書要約の faithfulness を compaction に転用できるかも不明である。
- constraint 脱落率と effort の関係を測った研究は見つからなかった。
- LoCoMo と LongMemEval は検索結果がアグリゲーターと vendor の自己申告に偏り、原典未確認のため数値を採用しなかった。

## Q4. benefit に対するコストはどの程度か

### Takeaway
low→max の増分は 1 サンプルで 2.03 倍(+964 output tokens)である。benefit の証拠がないので費用対効果は計算できない。実害になりやすいのは、token 額より latency と出力枠の消費に見える。

### Cited Findings
- 対象システムの測定は 1 会話の summary で low 934、max 1,898 output tokens。**1 サンプルのみ**で、依頼文の値であり再測定していない。2.03 倍と +964 は計算値で、reasoning と可視 summary の内訳は不明。
- summariser が agent の reasoning モデルで動くため、compaction 1 回の median は 161 s(p90 263 s)。60 ターンまたは 45 分の予算下では compaction ありの差は −3.3 pp(CI −10.8〜+4.2、p=0.48)、45 分上限到達は 51 vs 39 — [Bud ContextCompact 2.0](https://arxiv.org/html/2610.25516v1)
- reasoning off の DeepSeek v4.1 Flash は median 2.8 s、fact retention 0.70、needed ids 0.93 で、他の arm に +0.13〜+0.16 の差。pilot 費用は $4.19、査読なし — [marina PR #529](https://github.com/h2oai/marina/pull/529)
- compaction なしの 200 セッション中 102 が、1 ターンの reasoning で window を使い切って終了した(compaction ありは 85)— [Bud ContextCompact 2.0](https://arxiv.org/html/2610.25516v1)
- compressor の費用は per example で gpt-4.1 $0.045、gpt-4.1-mini $0.014(−69.2%)、distilled Qwen3-14B $0.0004(−99.1%)。distill 後も teacher の 95% を保持。end-to-end は $0.331→$0.285 / タスク、latency は 73.24 s→87.68 s — [ACON, Table 4](https://arxiv.org/html/2510.00615)
- reasoning を一度だけ使う代替案として、o3 による guideline 最適化はベンチマークあたり $2 未満(rollout は約 $20)。ただし o3 の優位は gpt-4.1 に +3.6、gpt-5 に +0.6 で、きれいな結果ではない — [ACON, Table 3](https://arxiv.org/html/2510.00615)
- 8K budget では Terminus-2 summarization $0.60 vs CliffCompaction $0.25 / インスタンス — [CliffCompaction](https://arxiv.org/html/2609.26779)

### Inferences
- +964 tokens は、2610.31778 の最小閾値 160K の約 0.6% に相当する。これは token 数の比で価格ではなく、1 サンプルに基づく粗い目安である。
- 増分が reasoning なら一回限りだが、可視 summary の増加なら後続ターンの入力に毎回乗る。今の測定では区別できない。
- 暫定的には、summary の effort は低めを既定にし、投資は constraint の分離と guideline に回すのが妥当に見える。これは直接証拠ではなく、間接証拠と不在からの推論である。モデル依存が大きいので、対象システムで A/B を取るまでは確定できない。
- effort を上げる場合は出力上限も同時に広げないと、summary の切断や fallback を招く恐れがある。

### Gaps
- 高 effort の benefit を downstream で定量した研究がなく、token あたりの費用対効果は算出できない。対象システムの測定を増やし、reasoning と可視 token を分離する必要がある。
- 対象システムの単価と latency は未確認のため、金額換算はしていない。
- **出典の確認状況**
  - 開始 4 件は実在を確認した: [2510.00615](https://arxiv.org/abs/2510.00615)、[2503.13657](https://arxiv.org/abs/2503.13657)、[2412.21187](https://arxiv.org/abs/2412.21187)、[2507.14417](https://arxiv.org/abs/2507.14417)。存在を確認できなかった指定 ID はない。
  - [2608.11242](https://arxiv.org/abs/2608.11242) は版で内容が異なる。v2 でタイトルが「Lost in Compaction: Evaluating Side-Constraint Loss under Context Compaction」から変わり、benchmark 名が CompInt から ConPACT に、extractor の Hermes retention が 95.6 から 93.7 になった。本ノートは v2 の値を採用した。
  - 未確認のため不採用: 2608.11242 の「GPT-5.4-mini 実験 約 800 USD」、2607.28802 の case study、CatalyzeX 掲載の goal drift 後続論文(タイトル・著者未確認)、2504.08120(本文未読)。
  - BrowseComp-Plus 70.5 / OOLONG-Pairs 0.1 は [2610.31778](https://arxiv.org/html/2610.31778) による引用のみで、RLM の原表では未確認。
  - 表の不整合の可能性: 2512.03503 の Table 6(COT 行、o1 行)、2507.02145 の QMSum ALL 行が Committee 行と同一。いずれも WebFetch 抽出時の指摘で、未検証。

---

**Not checked:** the search covered a few query families, so "no direct evidence" means none found, not none exists. Table values marked [行未照合] were confirmed to appear in the paper but not matched to their table rows.
