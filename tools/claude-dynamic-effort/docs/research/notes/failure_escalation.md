# 失敗シグナルでの reasoning effort 引き上げと recovery 後の引き下げ: エビデンス横断調査

## 外部失敗(tool error、failing test)後に reasoning / model strength を上げると結果は改善するか。コストは

### Takeaway
「同一モデルの effort を外部失敗シグナルで上げる」policy を直接評価した一次研究は見つからなかった。近い証拠を総合すると、model strength の escalation は安価だが精度は最強モデル単独に届かない。走行途中の escalation は、handoff の方式次第で最初から強モデルを使うより高くつくことがある(Handoff Tax)。失敗後に計算量だけを足す効果は小さく、診断情報を足す効果のほうがはるかに大きい(ToolBench-X)。

### Cited Findings

#### シード論文の実在確認(全9件を arXiv で確認済み、2026-11-05 時点)
- 2603.07915 = "Ares: Adaptive Reasoning Effort Selection for Efficient LLM Agents" — [Source](https://arxiv.org/abs/2603.07915)
- 2303.11366 = "Reflexion: Language Agents with Verbal Reinforcement Learning" — [Source](https://arxiv.org/abs/2303.11366)
- 2310.01798 = "Large Language Models Cannot Self-Correct Reasoning Yet"(Huang et al., ICLR 2024) — [Source](https://arxiv.org/abs/2310.01798)
- 2304.05128 = "Teaching Large Language Models to Self-Debug" — [Source](https://arxiv.org/abs/2304.05128)
- 2510.22977 = "The Reasoning Trap: How Enhancing LLM Reasoning Amplifies Tool Hallucination"(ACL 2026 Main) — [Source](https://arxiv.org/abs/2510.22977)
- 2509.09853 = "SWE-Effi: Re-Evaluating Software AI Agent System Effectiveness Under Resource Constraints" — [Source](https://arxiv.org/abs/2509.09853)
- 2503.13657 = "Why Do Multi-Agent LLM Systems Fail?"(MAST) — [Source](https://arxiv.org/abs/2503.13657)
- 2506.04210 = "Does Thinking More always Help? Mirage of Test-Time Scaling in Reasoning Models"(NeurIPS 2025) — [Source](https://arxiv.org/abs/2506.04210)
- 2504.15895 = "Dynamic Early Exit in Reasoning Models"(DEER) — [Source](https://arxiv.org/abs/2504.15895)

#### 比較表: 各研究が実際に操作しているもの
| 研究 | モデル | タスク | 指標 | 効果量 | N | trigger / 操作 |
|---|---|---|---|---|---|---|
| [AI Agents That Matter](https://arxiv.org/abs/2407.01502) | Llama-3 8B → GPT-3.5 → Llama-3 70B → GPT-4 | HumanEval | accuracy / 総コスト | Escalation 85.0% / $0.27、GPT-4 89.6% / $1.93、Retry (GPT-4) 92.0% / $2.51 | 164問 × 5 runs | example test 失敗で model を上げる(failing test 起点) |
| [Handoff Tax](https://arxiv.org/abs/2608.24358) | Haiku 4.5 → Opus 4.7、GPT-5.6 Luna → Sol | SWE-bench Verified | pass率 / 1タスクのコスト | Claude: LC 60.7% / $0.40、HC 79.2% / $0.72、Raw 69.2% / $1.61。GPT: LC 58.7% / $0.06、HC 83.7% / $0.47、Raw 67.5% / $0.36 | 500タスク、58,000 runs | 固定 switch point。失敗起点ではない |
| [Mehta](https://arxiv.org/abs/2607.02436) | Opus 4.7 High vs xHigh | 同一仕様のアプリ構築 | first-try perfect率 | 5/18 (28%) → 16/18 (89%)、差 61pt(95% CI 29–78)。コスト増は中央値で base 約9% | 全90 runs、対比は各18 runs | 開始時から effort 固定。条件はランダム化なし |
| [ToolBench-X](https://arxiv.org/abs/2606.25819) | 5モデル(全体では12モデル) | 例外注入の multi-step tool タスク | accuracy | TTS +3.5–11.5pt、Hint +25.5–35.5pt | 200タスク(全体 1,106) | 失敗後の追加 inference round と診断 hint |
| [Ares](https://arxiv.org/abs/2603.07915) | gpt-oss-20b + Qwen3-1.7B router | TAU-Bench、BrowseComp-Plus、WebArena | 成功率 / reasoning tokens | Retail RL: 54.8 → 58.5%、1007k → 476k tokens(52.7%減、表値から算出) | 表の Ttotal/Ttask から約115/50/150/129タスクと逆算 | step 単位の学習 router。明示的な失敗 trigger なし |
| [Self-Debug](https://arxiv.org/abs/2304.05128) | Codex、GPT-3.5、GPT-4、StarCoder | Spider、TransCoder、MBPP | accuracy | GPT-4: TransCoder 77.3 → 90.4、MBPP 72.8 → 80.6、Spider 73.2 → 73.6 | データセット単位(件数は未抽出) | 実行結果 feedback で再生成。effort は不変 |
| [Reflexion](https://arxiv.org/abs/2303.11366) | GPT-4 ほか | HumanEval、AlfWorld、HotPotQA | pass@1 / 解決数 | HumanEval (PY) 80.1 → 91.0、AlfWorld 130/134 | AlfWorld 134 env | 失敗後の verbal reflection。effort は不変 |
| [GMI blog](https://www.gmicloud.ai/en/blog/dynamic-model-escalation-for-coding-agents-a-gmi-router-experiment-across-two-harnesses) | DeepSeek V4 Flash ↔ Claude Fable 5 | Terminal-Bench 2.1 | 救済数 / 誤発火 | hard 9タスク中3救済、control 6タスクは全合格だが2件が誤 escalation | 9 + 6タスク、各1 trial | P(fail) と loop/overrun で昇格、2連続 safe step で降格(vendor blog、査読なし) |

#### 主要な知見
- Escalation の 85.0% は GPT-4 単独の 89.6% を下回る。著者は、cheap model の解が example test を通るのに evaluation test で落ちる場合があるためと説明している — [Source](https://arxiv.org/abs/2407.01502)
- 著者の "strictly improves accuracy" は LDB (GPT-3.5)(80.2%、$0.63)との比較であり、GPT-4 との比較ではない — [Source](https://arxiv.org/abs/2407.01502)
- Claude pair の Raw escalation($1.61)は、LC を捨てて HC で再開する Abort + HC fresh($0.90)より高く、品質も低い(69.2% vs 79.2%)。著者は strictly dominated と表現している。ただし再開コントロールは実際に実行したものではなく matched run からの合成値で、品質は構成上 HC-only と同じになる — [Source](https://arxiv.org/abs/2608.24358)
- GPT pair では Raw($0.36)は HC-only($0.47)より安いが、quality gap の回収は 36% に留まる — [Source](https://arxiv.org/abs/2608.24358)
- 引き継ぐ文脈を減らすと escalation は改善する。Traj-drop の QRec は Claude 64%、GPT 84%、Compactpre の CSRet は Claude −11%、GPT 49% — [Source](https://arxiv.org/abs/2608.24358)
- Claude pair の escalation は easy/medium では不利だが、hard タスクでは reduced-context interface が HC-only より安くなり、quality gap の 65–74% を回収する — [Source](https://arxiv.org/abs/2608.24358)
- 要件が後から届く設定では順序が逆転し、escalation が HC の品質優位の 86% を回収する(downshift は 31%) — [Source](https://arxiv.org/abs/2608.24358)
- Mehta は effort 引き上げを支持する: corrective prompt は 0.89 → 0.17/run に減り、コスト増は base 9%、tool 併用 29%、design prompt 併用 21% — [Source](https://arxiv.org/abs/2607.02436); ただし同論文内の abridged prompt 条件では xHigh も High も 1/6 (17%) で差がない — [Source](https://arxiv.org/abs/2607.02436)
- Mehta の High 側の失敗は npm install や Docker など初回の環境失敗に集中しており、著者は「より多い deliberation が事前に捕まえる種類の欠陥」と解釈している(観察研究) — [Source](https://arxiv.org/abs/2607.02436)
- ToolBench-X は compute を弱い介入と結論している。GPT-5.4-Mini は Hint 0.840 vs TTS 0.520、Claude-Sonnet-4.6 は 0.775 vs 0.535 — [Source](https://arxiv.org/abs/2606.25819)
- Self-Debug の改善は unit test がある TransCoder / MBPP で最大 12%、unit test のない Spider では 2–3%(最難問で 9%) — [Source](https://arxiv.org/abs/2304.05128)
- Reflexion の HumanEval 91% — [Source](https://arxiv.org/abs/2303.11366); AI Agents That Matter の再測定では Reflexion (GPT-4) は 87.8%(86.0–90.9)、$3.90 で、同研究の GPT-4 単独 89.6%、$1.93 を下回る — [Source](https://arxiv.org/abs/2407.01502)
- Ares の router は WebArena で go_back と branch に high effort を最も多く割り当てた。著者はこれを誤り訂正や計画修正と解釈している(Figure 3、割合の数値は本文になし) — [Source](https://arxiv.org/abs/2603.07915)
- Ares の 52.7% は abstract の「up to」だが、Table 2 の Airline RL は 873k → 133k で、表値から算出すると 84.8% 減になる — [Source](https://arxiv.org/abs/2603.07915)
- Ares の SFT のみの削減は Retail 35.3%、Airline 22.3%、BrowseComp-Plus 41.8%、WebArena 45.3%(表値から算出) — [Source](https://arxiv.org/abs/2603.07915)

### Inferences
- 失敗後の改善の主因は effort より外部シグナルの情報量である可能性が高い。Self-Debug の unit test 有無の差、ToolBench-X の Hint と TTS の差が同じ方向を示す。
- effort の上げ方としては、失敗後の反応より開始時の設定のほうが支持材料が多い。Mehta は事前設定の効果であり、反応的 escalation の効果を示すものではない。
- escalation するなら failure trajectory をそのまま渡さない設計が候補になる。ただし Handoff Tax は model 間の handoff の結果であり、同一モデルで effort だけを変える場合に文脈の負担が同様に生じるかは未検証。

### Gaps
- 同一モデルで effort を失敗 trigger により変更し、固定 effort と比較した一次研究は見つからなかった。
- Handoff Tax の著者は固定 switch point を使っており、adaptive な policy は future work としている。失敗起点の switch で結果が変わるかは不明。
- Ares の評価対象は gpt-oss-20b で、frontier model の effort 階層への外挿は未検証。Airline は約50タスクなので 42.0 と 38.0 の差は2タスク相当(表値からの推定)。
- Ares の BrowseComp-Plus 行は Ttotal/Ttask が他行の約150と合わず約158になる。論文側の不整合の可能性があり、原因は未確認。
- GMI blog の数値はサンプルが小さく、Flash は自社実行、Fable は公式 trajectory で条件が揃っていない。

## retry 上限、方針変更、reasoning が割に合わなくなる点についての証拠

### Takeaway
実験で設定された上限は3〜10回前後に集まるが、上限値そのものを比較した研究は見つからなかった。同じ行動の retry は最頻の既定動作で効果は限定的であり、失敗種別に応じた方針変更は優位を示すものの、live 評価では外部 policy が負ける結果もある。reasoning 長は非単調で、単一 trace の延長は一定点を超えると精度を下げる。

### Cited Findings

#### 比較表
| 研究 | モデル | タスク | 指標 | 効果量 | N | 操作 |
|---|---|---|---|---|---|---|
| [Self-Debug](https://arxiv.org/abs/2304.05128) | Codex ほか | code generation | debug turn 数 | 上限10、成功はほぼ3 turn 以内 | 3データセット | 反復上限 |
| [Reflexion](https://arxiv.org/abs/2303.11366) | GPT系 | AlfWorld、WebShop | 解決数 | ReAct は trial 6–7 で頭打ち、Reflexion は12 trial で 130/134。WebShop は4 trial で打ち切り | 134 env、100 env | 3連続失敗で停止(HotPotQA) |
| [Huang](https://arxiv.org/abs/2310.01798) | GPT-3.5、GPT-4 ほか | GSM8K、CommonSenseQA、HotpotQA | accuracy | GPT-4 GSM8K: 95.5 → 91.5 → 89.0。oracle 付きは 97.5。GPT-3.5 CSQA: 75.8 → 38.1 → 41.8 | GSM8K 1,319、CSQA 1,221 | 外部 feedback なしの self-correct |
| [Mirage](https://arxiv.org/abs/2506.04210) | DeepSeek-R1-Distill-Qwen ほか | GSM-8K、MATH-500、AIME 2024 | accuracy | 82.2 → 87.3%(385 → 1100 tokens)→ 70.3%(15980 tokens) | 件数は未抽出 | "Wait" による test-time 延長 |
| [DEER](https://arxiv.org/abs/2504.15895) | 11 reasoning LLM | 10 benchmark | CoT 長 / accuracy | CoT −19.1〜−80.1%、accuracy +0.3〜+5.0% | 11モデル × 10 benchmark | 高確信で早期終了 |
| [ToolBench-X](https://arxiv.org/abs/2606.25819) | 12モデル | 例外注入 tool タスク | 失敗直後の行動 | 同一 tool retry 44–76%、即終了 12%未満 | 1,106タスク | 観察 |
| [Self-Healing](https://arxiv.org/abs/2606.01416) | 決定的 tool 中心 | fault injection | 成功率 | 98.8 vs retry-only 94.5 vs replan 93.8。1回のみ: 94.0 / 85.3 / 88.2 | 100タスク | 失敗種別ごとの recovery |
| [Recovery Is a Decision](https://neurips.cc/virtual/2026/166916) | open-weight 2種 | SciAgentGym ほか | recovery regret / 一致率 | replay では最良、live では全外部 policy が native 処理に劣る。一致率 63.6% vs 約43% | 16,163 events、約1,900 episodes | retry / switch / abort の triage |
| [Bench2Robust](https://arxiv.org/abs/2608.11977) | gap 評価は7モデル4系統。介入の数値は Qwen3-4B-Thinking-2507 | 注入ありの multi-turn tool タスク | pass率(代替 tool なし / あり) | base 20.1 / 31.9%、BTM 36.9 / 43.6%(+16.8 / +11.7pt)、BTM + RL 40.8 / 45.5%。gap は70組中69組で悪化 | held-out Retail 402タスク | 構造化 recovery 文脈と RL |
| [ToolMaze](https://arxiv.org/abs/2606.05806) | 6 open-weight ほか | DAG 型 tool タスク | PRR / TSR | failure-aware prompt +1.5〜+20.8%、implicit 失敗で PRR 約37%低下 | タスク数は未抽出 | 上限25 step、perturbation |
| [PROBE](https://arxiv.org/abs/2605.08717) | 未抽出 | SWE-bench、EnterpriseOps-Gym、AIOpsLab | 診断精度 / 回復率 | 65.37% / 21.79%(最良 baseline 比 +43.58 / +12.45pt) | 未解決 257件 | 修復1回の構造化診断 |

#### 主要な知見
- Reflexion の baseline(ReAct、CoT、CoT (GT))は temperature 0.7 の再試行で、初回に失敗した HotPotQA タスクを1件も解けなかった — [Source](https://arxiv.org/abs/2303.11366); 一方 HumanEval では Retry (GPT-4) が単独の 89.6% から 92.0% に上がる — [Source](https://arxiv.org/abs/2407.01502)
- Reflexion は WebShop で改善せず、著者は多様性と探索を要するタスクは解けないと結論している — [Source](https://arxiv.org/abs/2303.11366)
- Huang は oracle label で停止を決めると self-correction の改善が水増しされると指摘しており、GSM8K では GPT-3.5 が 74.7% で答えを維持し、残りでは正解を誤答に変えるほうが多い — [Source](https://arxiv.org/abs/2310.01798)
- Mirage の Figure 7(a)(DeepSeek-R1-Distill-Qwen-1.5B / GSM-8K)では、16000 tokens への延長で −11.8%、同予算の parallel thinking で +10.1% — [Source](https://arxiv.org/abs/2506.04210)
- Mirage は abstract で「up to 20%」、Figure 8 で 16K tokens 予算時に約22% / 47% と記載しており、値の基準が異なる — [Source](https://arxiv.org/abs/2506.04210)
- Mirage の著者は、精度がトークンに対して単調でないため信頼できる停止基準がないと述べている — [Source](https://arxiv.org/abs/2506.04210)
- DEER は約75%のサンプルに「pearl」(早期終了でも正解できる点)があると分析している — [Source](https://arxiv.org/abs/2504.15895)
- Ares では Airline で Medium 42.0 が High 38.0 を上回り、WebArena でも Ares 46.5 が High 45.0 を上回る。著者は overthinking と呼ぶ — [Source](https://arxiv.org/abs/2603.07915); ただし BrowseComp-Plus は Low 8.00、High 42.7 と effort への感度が高い — [Source](https://arxiv.org/abs/2603.07915)
- Self-Healing の結果は決定的 tool の統制実験で、著者も model-in-the-loop は本番 API 評価ではないと述べている — [Source](https://arxiv.org/abs/2606.01416); live 評価では abort が完走できた episode を捨てるため外部 policy が負け、abort を agent への委譲に変えると差の大半が戻る — [Source](https://neurips.cc/virtual/2026/166916)
- SWE-Effi では SWE-Agent + GPT-4o-mini の失敗 1回は 880万 tokens、658秒、成功 1回は 180万 tokens、167.2秒で、著者は futility detection の欠如と表現している — [Source](https://arxiv.org/abs/2509.09853)
- 失敗 trajectory は成功より長い: SWE-agent 12.6% / 18.5%、OpenHands 31.0% / 82.5%、Prometheus 56.6% / 50.7%(Lite / Verified) — [Source](https://arxiv.org/abs/2511.00197)
- 同研究は再帰の上限だけでは不十分で、非生産的な推論ループを放棄する仕組みが必要と結論している — [Source](https://arxiv.org/abs/2511.00197)
- SWE-Next では失敗の 87.4% が step budget 到達で終わり、env/harness 由来は 2.4%。失敗種別は patch quality が 75.7% — [Source](https://arxiv.org/abs/2603.20691)
- PROBE は診断精度 65.37% に対して回復率 21.79% で、著者は diagnosis–recovery gap と呼ぶ — [Source](https://arxiv.org/abs/2605.08717)

### Inferences
- reasoning が割に合わなくなる点は共通の値としては示せない。モデル、タスク、予算で変わるため、固定の閾値より、外部検証で進捗がなければ打ち切る設計のほうが証拠と整合する。
- 同じ行動の retry 上限は、一過性の失敗には有効で、持続的な失敗には無駄という種別依存で考えるのが妥当である。診断情報の効果が大きいことがこれを裏付ける。
- 外部 recovery 機構の評価は live 実行でなければ信頼しにくい。Self-Healing の高い数値は replay が過大評価する例と同じ構造の可能性がある(同一条件の比較ではない)。
- 診断しても修復に至らない例が多い(PROBE)ため、方針変更だけで回復するとは限らない。

### Gaps
- retry 上限の値そのものを無作為に変えて比較した研究は見つからなかった。3や5などの値は設計上の選択か観察に基づく。
- 「失敗後に effort を上げる」と「方針を変える」を同一実験で比較した研究は見つからなかった。
- Recovery Is a Decision は NeurIPS 2026 workshop の abstract のみ確認した。arXiv ID は見つからず本文は未読。
- Mirage の推論の知見は主に数学タスク由来で、agent の tool loop への適用は Ares の overthinking 観察が間接的に支えるのみ。

## 強い reasoning は tool hallucination などの failure mode を増やすか

### Takeaway
小規模モデルの単一 step 診断ベンチでは、reasoning 強化と tool hallucination の増加が一貫して観測される(Reasoning Trap)。ただし文脈付きの agent 評価では reasoning モデルのほうが hallucination が低い例もあり、結果は割れている。frontier model の effort 引き上げが hallucination を増やすことを示す直接証拠は見つからなかった。

### Cited Findings

#### 比較表
| 研究 | モデル | タスク | 指標 | 効果量 | N | 操作 |
|---|---|---|---|---|---|---|
| [Reasoning Trap](https://arxiv.org/abs/2510.22977) | Qwen3-8B | SimpleToolHalluBench | RNTA / RDT (%) | Think Off 4.1 / 36.2 → On 5.4 / 56.8 | 349 tools × 2設定 | thinking の on/off |
| 同上 | Qwen3-32B | 同上 | 同上 | 5.1 / 46.6 → 8.8 / 50.7 | 同上 | 同上 |
| 同上 | Qwen3-4B | 同上 | RNTA | 3.4 → 29.4 | 同上 | Instruct vs Thinking |
| 同上 | Qwen2.5-7B | 同上 | RNTA / RDT、reward | 34.8 / 54.7 → 90.2 / 100.0、reward 0.22 → 0.45 | 同上 | think-then-act RL |
| 同上 | Qwen2.5-7B | 同上 | 同上 | direct tool-use RL は 41.4 / 63.6、reward 0.28 | 同上 | 推論を外した RL |
| 同上 | DeepSeek-V3 → R1 | 同上 | RNTA / RDT | 10.8 / 33.8 → 17.6 / 42.6 | 同上 | モデル比較 |
| 同上 | Kimi-K2 → Thinking | 同上 | RNTA / RDT | 1.0 / 15.5 → 4.4 / 21.3 | 同上 | モデル比較 |
| [MIRAGE-Bench](https://arxiv.org/abs/2507.21017) | DeepSeek-chat vs reasoner | snapshot 評価 | 全体 HR / US | 0.362 / 0.549 vs 0.257 / 0.641 | 件数は未抽出 | モデル比較(評価は不完全と注記あり) |
| [TIM](https://arxiv.org/abs/2511.10899) | GPT-5、o4-mini、Gemini-2.5-Flash ほか | PyMath | tool 使用率 / TIM 率 | thinking TaLM は code 利用が約50%多い。高リスク例の 54.3% で TIM | 評価 1,000問 | Code Interpreter |
| [ThinkBrake](https://arxiv.org/abs/2510.00546) | LRM | 数学、tool 使用 | 精度 / tokens | oracle 停止で平均 +8%、−72% tokens、tool 使用では −89% | 未抽出 | 事後最適の停止 |

#### 主要な知見
- Reasoning Trap の因果の根拠は、推論ありの RL が推論なしの RL より大きく hallucination を増やす点にある(同じデータ、報酬、ハイパラ) — [Source](https://arxiv.org/abs/2510.22977)
- GSM8K での GRPO でも hallucination は上がるため、tool データへの過学習だけでは説明できない — [Source](https://arxiv.org/abs/2510.22977)
- 同時に BFCL Multi-Turn は 13.6 → 23.5 に改善し、IFEval は 62.4 → 59.8、ComplexBench は 60.8 → 59.4 — [Source](https://arxiv.org/abs/2510.22977)
- 緩和には代償がある。DPO は 55.8 / 71.4 に下げるが reward は 0.45 → 0.34、prompt は 87.5 / 98.9、reward 0.44 に留まる — [Source](https://arxiv.org/abs/2510.22977)
- 著者は benchmark が単一 step であることを限界に挙げている — [Source](https://arxiv.org/abs/2510.22977)
- MIRAGE-Bench では DeepSeek-reasoner が repetitive history(FIH-R)で HR 0.213、chat は 0.417 — [Source](https://arxiv.org/abs/2507.21017); ただし論文は DeepSeek 2種の評価が context window の制約で不完全と注記しており、この対比は強い反証にはならない — [Source](https://arxiv.org/abs/2507.21017)
- 同じ表で Claude-3.5-Sonnet から 3.7-Sonnet へは FIH-R の HR が 0.227 → 0.325 に悪化する一方、全体 HR は 0.308 → 0.290 に改善する — [Source](https://arxiv.org/abs/2507.21017)
- ToolBench-X では Qwen-3.5-35B の thinking 版が非 thinking 版より 4.7pt 高い — [Source](https://arxiv.org/abs/2606.25819); これは Reasoning Trap の方向と逆の指標だが、測定対象は hallucination ではなく accuracy である
- Ares は excessive reasoning の失敗例として action format の誤りやタスク焦点の喪失を挙げている(観察の記述で定量なし) — [Source](https://arxiv.org/abs/2603.07915)
- Strong-weak 研究でも Weak Router が Strong Router を上回る例があり、著者は強いモデルが routing を「overthink」している可能性を推測している(推測と明記) — [Source](https://arxiv.org/abs/2505.20182)
- ThinkBrake は正しい中間解を推論の継続で上書きする現象を overthinking と定義し、実用手法の削減は最大 30% — [Source](https://arxiv.org/abs/2510.00546)
- MAST の FM-2.6(reasoning-action mismatch)は 13.2% — [Source](https://arxiv.org/abs/2503.13657)

### Inferences
- Reasoning Trap は effort を上げると hallucination が増えることの証明ではない。対象は主に小規模モデルの RL と thinking の on/off で、Qwen3 の on/off の増分は 8B の RDT を除くと小さい。一方、tool 不在時の hallucination を警戒する根拠としては十分である。
- policy への含意は、effort を上げる場面ほど tool 呼び出しの実在性を harness 側で検証するという組み合わせである。これは証拠からの推論で、直接は未検証。
- 失敗直後は tool の異常や不在が実際に起きている可能性が高く、Reasoning Trap が測る状況(必要な tool がない)に近い。このため逆効果のリスクがありうるが、これも未検証。

### Gaps
- frontier model の effort 階層(High、xHigh など)で tool hallucination を測った研究は見つからなかった。
- 失敗発生後の step に限定して reasoning 強度と hallucination の関係を測った研究は見つからなかった。
- MIRAGE-Bench の FIH-R の snapshot 件数は抽出できなかった。

## verified recovery 後の de-escalation と repeat / loop detection の証拠

### Takeaway
「verified recovery を trigger に下げる」policy を評価した査読研究は見つからなかった。Handoff Tax は「強モデル先行、後から安いモデル」が有利なことを示すが、これは recovery を条件にしていない。loop detection は検出としては機能し失敗の予測にもなるが、SWE-smith では repeat を抑えても resolve rate は上がらなかった。

### Cited Findings

#### 比較表
| 研究 | モデル | タスク | 指標 | 効果量 | N | 操作 |
|---|---|---|---|---|---|---|
| [Handoff Tax](https://arxiv.org/abs/2608.24358) | Opus 4.7 → Haiku 4.5 | SWE-bench Verified | pass / コスト | LC 54.6% / $0.41 → Raw 65.6% / $0.51、HC 75.8% / $0.85、QRec 50、CSRet 80 | 500タスク | 固定点での downshift |
| 同上 | Sol → Luna | 同上 | 同上 | LC 63.6% / $0.05 → Raw 81.0% / $0.41、HC 85.8% / $0.47、QRec 79、CSRet 14 | 同上 | 同上 |
| [Strong-weak](https://arxiv.org/abs/2505.20182) | o3-mini + GPT-4o-mini | SWE-bench Lite | 解決率 / 総コスト | Strong LM First 0.3000 / $24.9、strong 単独 0.3367 / $46.2 | 300 issues、各1回 | 強モデルの後に弱モデルで整形 |
| [Khraishi](https://arxiv.org/abs/2603.03111) | 複数 | CoQA、Multi-IF | F1 / strict success | −8〜+13pt、±4 F1 | 72 off-diagonal 組 | 最終 turn のみ切替 |
| [SWE-smith](https://arxiv.org/abs/2504.21798) | SWE-agent-LM-32B、Claude 3.7 | SWE-bench Verified | 解決率 | 40.2% → 38.4%(192件) | 500件 | 警告と resample で repeat 抑制 |
| [Cycle Detection](https://arxiv.org/abs/2511.10650) | LangGraph アプリ | 株式 agent | F1 | 0.72(P 0.62、R 0.86)、単独手法は 0.08〜0.45 | 1,575 trajectories | 構造 + 意味の hybrid |
| [IAL-Scan](https://arxiv.org/abs/2607.01641) | 静的解析 | agent リポジトリ | precision | 74件中68件確定、91.9% | 6,549 repos | 無制限ループの検出 |
| [MAST](https://arxiv.org/abs/2503.13657) | GPT-4系、Claude系 | MAS traces | 失敗 mode の割合 | step repetition 15.7%、停止条件の不認識 12.4% | 1,642 traces(分類は150) | 観察 |

#### de-escalation
- Handoff Tax の downshift は HC trajectory を残すことが重要で、Traj-drop は Claude で QRec 28%(Raw 50%、Compactpre 56%)、GPT で 53%(Raw 79%)に下がる — [Source](https://arxiv.org/abs/2608.24358)
- 著者は「HC の trajectory は LC を導き、LC の trajectory は HC の負担になる」と要約している — [Source](https://arxiv.org/abs/2608.24358)
- 切替自体にコストがある: 単一 turn の切替でも CoQA で 22%(16/72)、Multi-IF で 25%(18/72)の組が 95% 水準で有意 — [Source](https://arxiv.org/abs/2603.03111)
- TwinRouterBench は tier 切替時の cache write を incoming tier の料金で計上しており、high tier の cache_write は cache_read の 12.5倍 — [Source](https://arxiv.org/abs/2605.18859)
- TACIT-Switch の著者は、一方向の永続 handoff では「短時間の強モデル支援の後に戻す」方が良い場合を逃すと述べている — [Source](https://arxiv.org/abs/2608.27911)
- GMI の実験は2連続 safe step で降格し、dwell、切替回数、時間の guard を置く。path-tracing では step 27 昇格、30 降格、33 再昇格と振動しつつ合格した — [Source](https://www.gmicloud.ai/en/blog/dynamic-model-escalation-for-coding-agents-a-gmi-router-experiment-across-two-harnesses)
- 同実験の control では fix-ocaml-gc が誤昇格後に降格して $0.93(Fable 単独 $1.08)、mailman は昇格したまま $1.51 — [Source](https://www.gmicloud.ai/en/blog/dynamic-model-escalation-for-coding-agents-a-gmi-router-experiment-across-two-harnesses)
- Ares の RL は Airline で high の使用を 50%超から 20%未満に、low を約70%に変え、42.0% を 133k tokens で達成した(SFT は 36.0%、678k) — [Source](https://arxiv.org/abs/2603.07915)

#### recovery の検証
- MAST の verification 失敗は incorrect 9.1%、no or incomplete 8.2%、premature termination 6.2%で、verifier が表層的な検査に留まる例を指摘している — [Source](https://arxiv.org/abs/2503.13657)
- Reflexion は flaky な自己生成テストが false positive を生むと認めている — [Source](https://arxiv.org/abs/2303.11366)
- Self-Healing の統制設定では verifier 付き recovery が silent failure を 0.0% に下げた(統制条件) — [Source](https://arxiv.org/abs/2606.01416)
- ToolMaze は implicit な失敗で PRR が約37%低下し、原因を破損出力への過信としている — [Source](https://arxiv.org/abs/2606.05806)

#### loop detection
- SWE-smith では長さ10の repeat が 89% の失敗確率に対応し、長さ10以上の repeat を持つ trajectory は SWE-agent-LM-32B で25%超、Claude 3.7 Sonnet で4%未満 — [Source](https://arxiv.org/abs/2504.21798)
- 同モデルの失敗の 53% は実行上限に関連し、repeat の 73% は `str_replace_editor view`(引数なしの base command で判定) — [Source](https://arxiv.org/abs/2504.21798)
- 反証: repeat を4/6回で警告し、6/8回で resample する介入は repeat を大きく減らしたが解決数は 192 (38.4%) に下がり、変種でも改善しなかった。著者は repeat を症状と解釈している — [Source](https://arxiv.org/abs/2504.21798)
- 設計例(効果は未評価): OpenDev は tool 名と引数の MD5 を直近20件で追跡し、3回で警告と実行スキップ、再発で承認待ちに移行する — [Source](https://arxiv.org/abs/2603.05344)
- 設計例(効果は未評価): 2609.20804 の harness は同一引数5連続で reminder、同一失敗8連続で終了とし、全条件で固定されている — [Source](https://arxiv.org/abs/2609.20804)
- 設計例(評価対象の抽出法): MIRAGE-Bench は同一行動の3回目と6回目で snapshot を取り、3回を人の試行錯誤の範囲を超える目安としている — [Source](https://arxiv.org/abs/2507.21017)
- Cycle Detection の単独手法は誤検知が多く、semantic のみでは recall 0.91 に対して precision 0.16 で、生産的な反復を誤判定する — [Source](https://arxiv.org/abs/2511.10650)
- IAL-Scan の確定68件の内訳は retry feedback 無制限 17件 (25.0%)、tool-call 反復無制限 16件 (23.5%)、multi-agent chat 無制限 14件 (20.6%) — [Source](https://arxiv.org/abs/2607.01641)

### Inferences
- de-escalation の trigger は self-report ではなく外部検証に限るのが妥当である。Huang、MAST、Reflexion、AI Agents That Matter の false pass はいずれも自己判定や弱い検証の信頼性の低さを示す。
- 切替コストがあるため、1回の成功で即座に下げると振動のリスクがある。dwell や回数の guard は GMI の設計例があるが、効果の比較検証はない。
- loop detection の最も支持された用途は、回復手段より早期打ち切りや予算保護の信号である。SWE-smith は「抑制しても解けない」ことを示すが、節約効果を測った研究ではないため打ち切りの利得は未確定。
- 引数込みの完全一致で判定する設計は、引数なしの base command で測る SWE-smith より誤検知が少ないと考えられるが、比較実験は見つからなかった。

### Gaps
- verified recovery を条件に effort や model を下げる policy の一次評価は見つからなかった。Handoff Tax の downshift は固定点での切替であり recovery の検証を伴わない。
- 同一モデルで effort のみを下げた場合の品質影響を、走行途中で測った研究は見つからなかった。
- loop 検出後に打ち切った場合のコスト節約と機会損失を測った研究は見つからなかった。
- MAST の割合の母数は本文の当該段落から確定できなかった(Figure 4 は 210 traces、taxonomy 構築は 150 traces)。
- GMI の dwell guard の具体値は確認できなかった。

## model cascade / router 研究は転用できるか

### Takeaway
部分的に転用できる。コスト削減の方向と「検証シグナルの質が結果を決める」点は一貫している。ただし query 単位の削減率(FrugalGPT の最大 98% など)は trajectory の状態を持たないため、そのまま agent に適用できない。step 単位や走行途中の研究では削減幅が小さくなるか、条件次第で負の値になる。

### Cited Findings

#### 比較表
| 研究 | モデル | タスク | 指標 | 効果量 | N | 粒度 / trigger |
|---|---|---|---|---|---|---|
| [FrugalGPT](https://arxiv.org/abs/2305.05176) | GPT-J、J1-L、GPT-4 ほか | HEADLINES、OVERRULING、COQA | コスト削減 | 98.3%、73.3%、59.2% | COQA 7,982 ほか | query / 学習済み scorer |
| [RouteLLM](https://arxiv.org/abs/2406.18665) | GPT-4、Mixtral-8x7B | MT Bench、GSM8K ほか | コスト | 2倍超の節約、MT Bench で random 比最大 75%減 | benchmark 単位 | query / 事前 routing |
| [Hybrid LLM](https://arxiv.org/abs/2404.14618) | Llama-2、GPT-3.5 ほか | NL query | 大モデル呼び出し | 最大 40%減 | 未抽出 | query / 難度予測 |
| [Model Cascading for Code](https://arxiv.org/abs/2405.15842) | 3系列 | HumanEval | コスト | 平均 70%、11%、17% 節約 | HumanEval | 問題 / 自己生成テスト |
| [TRIM](https://arxiv.org/abs/2601.10245) | 小 + 大モデル | MATH-500、AIME | コスト効率 | 閾値法 5倍、AIME で最大 6倍 | 未抽出 | step / PRM スコア |
| [AI Agents That Matter](https://arxiv.org/abs/2407.01502) | 4段 chain | HumanEval | accuracy / コスト | 85.0% / $0.27(GPT-4 は 89.6% / $1.93) | 164問 × 5 | 問題 / example test |
| [Strong-weak](https://arxiv.org/abs/2505.20182) | o3-mini + GPT-4o-mini | SWE-bench Lite | 解決率 / コスト | Weak LM First 0.1633 / $5.7(weak 0.1533 / $4.5)、Weak Router 0.2267 / $16.8 | 300 | 実行 / 無効 patch |
| [TACIT-Switch](https://arxiv.org/abs/2608.27911) | 9B → 27B | DABench | 成功 / コスト | 49/67 (73.1%) / 19.81(Pure 27B 46/67 / 38.59) | 67 | 走行中 / 累積リスク、永続 |
| 同上 | 4B → 27B | ALFWorld | 成功 / コスト | 65/134 / 191.10(Pure 27B 68/134 / 230.51) | 134 | 同上 |
| [BudgetMLAgent](https://arxiv.org/abs/2411.07464) | Gemini-Pro + GPT-4 | MLAgentBench | 成功率 / コスト | 32.95% vs GPT-4 単独 22.72%、コスト 94.2% / 87.1%減 | 未抽出 | step / retry 枯渇と planner の自己申告 |
| [TwinRouterBench](https://arxiv.org/abs/2605.18859) | Opus ほか | SWE-bench ほか | コスト | 学習 router で Opus 比 53.1%減($25.66 vs $54.73) | 100ケース | step / 学習 |

#### 主要な知見
- Strong-weak の Weak LM First は有効な patch を生成できない場合のみ強モデルを呼ぶため、強モデル使用は 0.08回に留まり改善は約1pt — [Source](https://arxiv.org/abs/2505.20182)
- 同研究の Strong LM First は strong 単独に近い性能を約60%のコストで達成し(O3-mini + Qwen2.5-Coder-32B)、abstract は 40% 削減と報告している — [Source](https://arxiv.org/abs/2505.20182)
- 著者は予算で最適手法が入れ替わると結論しており、低予算では Weak LM First、予算増で Strong LM First が優位になる — [Source](https://arxiv.org/abs/2505.20182)
- TACIT-Switch の統制シミュレーションでは、同コストで Task Router 62.40%、Step Deferral 62.98%、SWE-Router 型 66.13%、TACIT 73.52% — [Source](https://arxiv.org/abs/2608.27911)
- TACIT の実データでは step 局所 deferral が ALFWorld 4B で 31/134 と低く、著者のコストは parameter 重み付き call 数という代理指標 — [Source](https://arxiv.org/abs/2608.27911)
- TwinRouterBench は one-shot の router 評価では途中 step の downstream 成功を検証できないと批判している — [Source](https://arxiv.org/abs/2605.18859)
- 同研究では rule-based の ClawRouter が SWE-bench で 38/40 失敗、UncommonRoute (rule-based) が 39/40 失敗 — [Source](https://arxiv.org/abs/2605.18859)
- BudgetMLAgent は planner に「詰まったと判断したら GPT-4 を呼ぶ」lifeline を与え、呼び出しには上限を設ける — [Source](https://arxiv.org/abs/2411.07464)
- ToolMaze の fit では、model 規模が1桁増えるごとに TSR(NP) は +17.85pt、PRR は +4.88pt で、差は 3.66倍(open-weight 6モデルの log-linear fit) — [Source](https://arxiv.org/abs/2606.05806)
- SWE-Effi は scaffold と model の組み合わせ依存を示す。SWE-Agent は Qwen3-32B で解決率 28%、35.5 calls、GPT-4o-mini で 10%、181 calls(50 issues、15 組) — [Source](https://arxiv.org/abs/2509.09853)
- GMI の実験では escalation 候補タスクの harness 間 Jaccard は 0.35 で、harness で候補が変わる — [Source](https://www.gmicloud.ai/en/blog/dynamic-model-escalation-for-coding-agents-a-gmi-router-experiment-across-two-harnesses)

#### 転用可能性の整理
- 転用しやすい: 検証シグナルの質が精度を縛る点(example test の false pass、無効 patch という弱い trigger) — [Source](https://arxiv.org/abs/2407.01502); [Source](https://arxiv.org/abs/2505.20182)
- 転用しにくい: query 単位の削減率。FrugalGPT は単一 query のテキスト分類や QA が対象で、trajectory の継承コストがない — [Source](https://arxiv.org/abs/2305.05176); 走行途中の切替は CSRet が −285% まで悪化しうる — [Source](https://arxiv.org/abs/2608.24358)
- 要注意: model の階層差と effort の階層差は別物である。Handoff Tax の Claude pair は HC/LC のコスト比が約2倍、GPT pair は約8倍で、結果の傾向が異なる — [Source](https://arxiv.org/abs/2608.24358)

### Inferences
- cascade 研究が effort policy に与える最も確かな教訓は、昇格の前提となる検証シグナルの質と、上位階層に渡す文脈の設計である。
- 同一モデルの effort 変更は model 切替より切替コストが小さいと考えられるが(履歴の出自が変わらない)、この点を測定した研究は見つからなかった。
- コスト比が小さい階層では「最初から上位を使う」が優位になりやすい(Claude pair の結果)。Mehta の xHigh の小さなコスト増と合わせると、effort の差が安い場合は動的 policy の節約余地自体が小さい可能性がある。

### Gaps
- 以下は一次資料で確認できず、本ノートでは数値を採用しなかった:
  - Darwin Cascade(GLM → Opus、51.33%、$0.27/instance)は GitHub の自己報告のみで未読
  - RouteLLM の「3.66倍」「85%削減」は二次資料の記載で、論文本文の抽出では確認できず(論文は2倍超、random 比最大 75%)
  - FrugalGPT の「平均約50%」は二次資料のみ
  - TRIM の「80%削減」は本ノートの抽出では確認せず(最大 6倍と閾値法 5倍は確認済み)
  - BudgetMLAgent の lifeline 上限「5回」は二次資料のみ
  - Claude Code の adaptive thinking に関する個人ブログ分析(17,871 thinking blocks など)は査読なしの単一利用者分析で、一次の検証ができないため除外
- 全般の限界:
  - 2026年の論文の多くは査読前の preprint であり、Handoff Tax、TACIT-Switch、ToolBench-X、ToolMaze、Self-Healing、Mehta は再現を確認していない
  - Budget-Aware Agentic Routing ([2602.21227](https://arxiv.org/abs/2602.21227)) と AutoMix ([2310.12963](https://arxiv.org/abs/2310.12963)) は存在を確認したが、数値は抽出していない
  - 一部の N(Hybrid LLM、TRIM、BudgetMLAgent、ToolMaze、SWE-Next)は本文から抽出できなかった
  - 引用文献を網羅的に追跡してはおらず、見つからなかったことは存在しないことを意味しない
