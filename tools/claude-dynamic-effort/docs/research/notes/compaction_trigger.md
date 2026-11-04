# long-running LLM agent における compaction の trigger タイミング（横断比較）

調査日: 2026-11-05。数値は primary source の本文・公式 docs・公開ソースコードで確認できたものだけを載せた。確認状況の一覧は Q1 の Gaps にある。vendor の既定値はすべて「実務（practice）」であり、根拠（evidence）ではない。

## 1. trigger タイミングを直接比較した evidence は何か。どこに無いのか

### Takeaway
閾値を直接振った研究は存在する（Beyond Token Savings、Benchmarks Are Surprisingly Compaction-Friendly、Bud ContextCompact 2.0、Parallel Context Compaction、ACON の ablation）。ただし比較範囲は最大 380K tokens までで、870K/967K 域、task boundary と閾値の直接比較、turn-end と in-turn の比較は見つからなかった。1M window の frontier model では 160K〜380K の間で性能差が測れず、小さい window では早すぎる trigger が性能を下げる、というのが現時点の最良の要約である。

### Cited Findings

#### 戦略別の横断表

「−」は該当なし、「未抽出」は本調査で読み取っていない値（推定はしていない）。

| 研究 | 戦略分類と設定 | モデル | タスク | 指標 | 効果量・戦略別の性能 | コスト | sample size |
|---|---|---|---|---|---|---|---|
| [Beyond Token Savings (2609.32961)](https://arxiv.org/abs/2609.32961) | 固定絶対閾値 10K/15K/20K（peak context 分布の下位パーセンタイルで較正）、D=0.5 | Qwen3.5-35B-A3B | SWE-bench Verified | resolve rate、推定 billed cost、latency | SU: 10K→20K で 50.7%→62.7%。SS-p: 10K/15K/20K で 50.7/60.0/73.3%。Devstral は SS-p で 72〜73% と変化なし | SU の input cost 1.10×→0.92×（対 full context）、latency 653s→599s、頻度 4.85→1.15 回/attempt | sweep は 25 tasks×3 attempts。全体は 65 policies、約 35,000 runs |
| 同上 | step trigger（毎 step の tool-result clearing, OTRC）vs 閾値（TRC） | Qwen / Devstral / GLM-4.7-Flash | SWE-bench Verified | resolve rate、billed cost | OTRC−TRC = −2.3 / −14.3 / +2.7 pp。Qwen は TRC 53.7% vs OTRC 51.3% | Qwen の billed cost 0.71× vs 0.95×、latency 0.79× vs 0.88×。step trigger は呼び出しが 10〜27% 増 | SWE-bench 100 issues、Terminal-Bench 40 tasks、各 3 回 |
| [Benchmarks Are Surprisingly Compaction-Friendly (2610.31778)](https://arxiv.org/abs/2610.31778) | 固定絶対閾値 160K〜380K（1M 窓の 16〜38%、400K 窓の 40〜95%） | Claude Opus 4.6、GPT-5.4（1M）、GPT-5.3-Codex（400K）、Gemini 3.1 Pro Preview | PaperBench、ProgramBench、PostTrainBench（OpenCode 上） | valid submission 条件付き accuracy | 1M の 2 モデルは閾値で差なし。GPT-5.3-Codex は PaperBench で 160K の 0.38→260K 以上で約 0.60、ProgramBench は平坦、PostTrainBench は CI が重なり結論不可。Gemini はほぼ compact せず情報なし | 本調査の抽出範囲ではトークン費用の比較なし | 1 task・1 構成あたり 2 runs、CI は bootstrap 95% |
| [Bud ContextCompact 2.0 (2610.25516)](https://arxiv.org/abs/2610.25516) | 固定絶対閾値 24K/32K/48K vs off（56,320 tokens 窓） | Qwen3.8-27B | SWE-bench Verified | resolve 数 | off 18、24K 21、32K 19、48K 16（各 /50）。long pairs の差は +13.9（p=0.18）/ 0.0（p=1.0）/ +2.4（p=1.0）で有意差なし | −（閾値別のコストは未報告） | 25 tasks×2 repeats、各 50 sessions。compact 発生は 20/16/4 sessions |
| 同上（main study） | 閾値 22.4K vs off（49K 窓） | Qwen3.8-27B | 同上 | resolve rate | long session で 41%→53%（+12.3 pp、95% CI +1.6〜+23.0、p=0.043）、short は +1.7 pp。45 分予算下では −3.3 pp（p=0.48） | 総トークン 108.4M vs 108.7M、解決 session 当たり 1.26M vs 1.10M（−13%）、model time 73.8h vs 84.2h | 100 tasks、556 graded sessions |
| [Parallel Context Compaction (2605.23296)](https://arxiv.org/abs/2605.23296) | 固定絶対閾値 16k/32k/64k/96k | gpt-oss-20B、Llama-3.1-8B | HotpotQA の agentic flow | compact 回数、wall time 比率 | 回数 15/7/3/2 | compact が E2E wall time に占める比率: 51.3/28.2/14.5/8.6%（gpt-oss）、62.4/53.6/24.4/14.5%（Llama） | 未抽出 |
| [ACON (2510.00615)](https://arxiv.org/abs/2510.00615) | 固定絶対閾値の ablation（history 4096、observation 1024 が最良と報告） | gpt-4.1 | AppWorld | accuracy、compress 頻度 | 小さい閾値は頻度増と accuracy 低下、大きい閾値は accuracy 維持でコスト高（値は Figure のみで本文に数値なし）。別途、全体で peak tokens −26〜54% | 未抽出 | 未抽出 |
| [Slipstream (2605.08580)](https://arxiv.org/abs/2605.08580) | 閾値は平均 trajectory 長の約 20% を基準に 3 水準。同閾値で同期 vs 非同期 | Qwen3.5-9B、Seed-OSS-36B-Instruct | SWE-bench Verified、BrowseComp | accuracy、latency | 非同期+検証で +2.6〜8.8（SWE）、+1.3〜4.6（BrowseComp）。閾値に対する単調傾向なしと著者が明記 | latency −11.3〜39.7% | 未抽出 |
| [SelfCompact (2606.23525)](https://arxiv.org/abs/2606.23525) | adaptive（モデル判断＋rubric。sub-task 解決時に発火）vs 窓比率 30% vs compact なし | GLM-4.7-Flash、MiniMax-M2.5、MiMo-V2-Flash（他に Qwen 4 種を数学で） | BrowseComp-Plus ほか計 6 benchmarks | accuracy | 対 baseline +8.5/+9.2/+5.3 点。対固定 30% は最大 +6.3 点。最難 2 bin では対閾値 5〜20 pp 改善 | 質問当たり費用 −67/−63/−33%（対 compact なし、全 prompt を cache 単価で計算する近似） | 1 benchmark 150 問 |
| [The Complexity Trap (2508.21433)](https://arxiv.org/abs/2508.21433) | 固定 turn 数（masking M=10、summary N=21, M=10） | Qwen3-Coder 480B ほか計 5 構成 | SWE-bench Verified（SWE-agent） | solve rate、instance cost | Raw 53.4%、masking 54.8%、summary 53.8% | $1.29、$0.61（−52.7%）、$0.64（−50.4%）。summary 呼び出しは総費用の最大 7.2% | n=500、paired bootstrap B=10,000 |
| [AgentDiet (2509.23586)](https://arxiv.org/abs/2509.23586) | 毎 step の sliding window 削減（θ=500 tokens、a=2） | agent に Claude 4 Sonnet（Trae Agent）。reflection に Claude 3.5 Haiku、Gemini 2.5 Flash など。評価全体の LLM 構成は未抽出 | SWE-bench Verified ほか | Pass%、cost | 全 variant が Pass% 58% 以上（Original は 65%） | input −39.9〜59.7%、agent cost −28.6〜44.1%、reflection 込みの総 cost −21.1〜35.9% | 未抽出 |
| [CliffCompaction (2609.26779)](https://arxiv.org/abs/2609.26779) | 固定絶対閾値 45K/32K/16K/8K（rule-based で要約なし） | Kimi K2.6、GLM 5.1 ほか | SWE-bench Verified、Terminal-Bench 2.0 | 成功率 | Kimi SWE-bench: full 73.87%、32K 73.27%、16K は 2.0 pp 以内。Terminal-Bench: full 59.16%、32K/16K とも 61.42%（同 16K で LLM 要約は 55.45%）。8K は低下 | 最大 −50%、cache read 費用は最大 −90% | 未抽出 |
| [Beyond Compaction / CWL (2606.11213)](https://arxiv.org/abs/2606.11213) | 構造化 eviction、上限 τ=80K（モデル窓の約 30%） | 未抽出 | Terminal-Bench 2.0（89 tasks）、SWE-bench Lite 50 tasks ほか | task accuracy | 1 session で 89 tasks・8,000 万 tokens を連続実行し、分離 session 比で測定可能な劣化なしと報告 | 無制限 session 比 −20〜70%、compaction 比 −23% | 上記 |

#### 分類ごとの evidence の有無
- **固定絶対閾値**: evidence は最も多い。ただし 8K〜380K の範囲に限られる（上表）。
- **窓比率**: 比率そのものを独立変数にした比較は見つからなかった。2610.31778 は絶対値で振り、比率は結果の解釈として述べるだけである。また「compaction 頻度はモデルごとの context 増加速度で決まるため、閾値はモデル間で比較できない」と明記している — [2610.31778](https://arxiv.org/abs/2610.31778)。SelfCompact の 30% は比較対照の 1 点である — [2606.23525](https://arxiv.org/abs/2606.23525)。
- **adaptive**: 閾値との直接比較は SelfCompact のみで、タスクは探索と数学である。coding は含まない — [2606.23525](https://arxiv.org/abs/2606.23525)。
- **task boundary**: 閾値との直接比較は見つからなかった。Complexity Trap は、自身の trigger が semantic boundary や subgoal を無視する非 adaptive なものだと limitation に挙げている — [2508.21433](https://arxiv.org/abs/2508.21433)。Claude Code docs の「natural break で /compact」は practice である — [Claude Code prompt caching](https://code.claude.com/docs/en/prompt-caching.md)。
- **最重要の欠落**: 2610.31778 は、自身の sweep が 1M 窓の 16〜38% であり「scaffold の既定値付近を検証していない」と明言する。同論文は scaffold の通常 trigger を窓の 90〜95% 付近と記す — [2610.31778](https://arxiv.org/abs/2610.31778)。
- **MAST** は multi-agent 失敗分類で、trigger の比較は含まない。関連するのは FM-1.4 Loss of conversation history（2.80%）のみである — [2503.13657](https://arxiv.org/abs/2503.13657)。

### Inferences
- 対象システムの 870K は、1M 窓における比較研究の外側にある。「870K で性能が落ちる」という evidence も「870K で問題ない」という evidence も無い。
- 167K は 2610.31778 の最低水準 160K に近い。Opus 4.6 と GPT-5.4 に限れば、この水準でも性能差は測定されていない。Fable や Opus 5.5 など sweep に含まれないモデルへの適用は外挿である。
- 2609.32961 の 10K→20K の改善は小型 open-weight モデルの話で、閾値も peak context の下位パーセンタイルである。「遅いほど良い」を 1M 窓へ外挿してはいけない。
- 閾値の感度はモデル依存が大きい（Qwen は 50.7→73.3%、Devstral は 72〜73% で不変）。自システムの A/B なしに数値を移植できない。

### Gaps
- 380K を超える閾値の比較研究は見つからなかった。
- turn-end（between-turn）compaction と in-turn compaction の性能比較は見つからなかった。
- task boundary trigger と閾値 trigger の coding task での直接比較は見つからなかった。
- 確認状況:
  - **arXiv 上で実在を確認**: 2510.00615、2503.13657、2508.21433、2509.23586、2609.32961、2610.31778、2610.25516、2605.23296、2608.01326、2609.00006、2604.03515、2606.22528、2608.22752、2607.08032、2608.06503、2603.17781、2609.26779、2606.23525、2605.08580、2606.11213、2609.11024。今回調べた ID で実在しないものはなかった。
  - **ID と投稿日の不整合**: 2609.00006 は v1 が 2026-07-15、2606.11213 は v1 が 2026-05-01 で、ID の年月より早い。abs ページのメタデータも同じ日付で、理由は不明である。
  - **未確認（本文を開いていない）**: SelfCompact が 30% の根拠として引く Liu et al. (2025)、context rot の Hong et al. (2025)、2610.31778 が引く Zhang et al. (2025) の BrowseComp-Plus 70.5 / OOLONG-Pairs 0.1。いずれも二次引用である。
  - **検索要約にあったが primary で確認できず、採用しなかった**: AgentDiet の「success rate −1.0〜+2.0%」。
  - **再検証**: 共有 scratchpad のファイルが他の調査者の抽出で上書きされた可能性があるとの連絡を受け、全論文を専用ディレクトリへ再取得し、主要な数値の文字列を照合した。不一致はなかった。2608.06503 は再取得時に v2（2026-10-14）へ更新されていたが、引用した値は同じである。
  - 全 21 本の大半は査読前の preprint である。AgentDiet は FSE 2026 採択と arXiv コメントに記載。Complexity Trap は NeurIPS 2025 DL4Code workshop と著者 repo に記載。他の査読状況は未確認である。
  - 論文は全文 HTML を取得したが通読はしておらず、キーワードと該当節の抽出で読んだ。docs の一部（compaction-threshold、cookbook、OpenAI compaction）は要約付き fetch を介して読んだ。

## 2. 早すぎる compaction と遅すぎる compaction のコスト

### Takeaway
早すぎるコストは、回数増に比例する損失（要約生成、prefix の書き直し、再探索、制約の喪失）である。遅すぎるコストは、毎 request の cache read 増、cache miss 時の巨額な再書き込み、窓枯渇である。どちらが支配的かはモデルと窓の大きさに依存し、普遍的な最適点は示されていない。

### Cited Findings

#### 早すぎる場合
- **回数と品質**: 10K→20K で頻度 4.85→1.15 回に減り、全 primitive で resolve rate が改善した — [2609.32961](https://arxiv.org/abs/2609.32961)。GPT-5.3-Codex は 160K で 0.38、260K 以上で約 0.60 だった — [2610.31778](https://arxiv.org/abs/2610.31778)。
- **要約コスト**: summary 呼び出しは総費用の最大 7.2%。cache 再利用は要約用 system prompt に限られた — [2508.21433](https://arxiv.org/abs/2508.21433)。要約呼び出しは solved run の wall-clock の 28〜46% を占めた — [2609.32961](https://arxiv.org/abs/2609.32961)。1 回の要約は中央値 161 秒（p90 263 秒）で、45 分予算下では利得が消えた — [2610.25516](https://arxiv.org/abs/2610.25516)。16k では wall time の最大 62.4% を占めた — [2605.23296](https://arxiv.org/abs/2605.23296)。
- **cache 無効化**: prefix を頻繁に書き換える policy は、トークンが減っても fresh 単価の割合が増え、請求が同等以上になりうる。0.6× のトークンで 0.71〜0.95× の billed cost だった（cached=0.1× を仮定した推定）— [2609.32961](https://arxiv.org/abs/2609.32961)。AgentDiet は input −39.9〜59.7% に対し cost −28.6〜44.1% で、著者は output 費用と無効化された KV cache を差の原因に挙げる — [2509.23586](https://arxiv.org/abs/2509.23586)。eviction が毎 turn 起きる高負荷では caching が純損失になりうる — [2606.11213](https://arxiv.org/abs/2606.11213)。
- **低頻度なら cache 損失は小さい**: trace からのモデル計算で prefix 再利用可能率は 98% vs 96% だった — [2610.25516](https://arxiv.org/abs/2610.25516)。
- **再読込・再探索**: Claude Code は compaction 直後に最大 5 ファイルを再読込する（5,000 tokens 超は path 参照のみ）— [Claude Code context window](https://code.claude.com/docs/en/context-window.md)。Gemini 2.5 Flash で summary は平均 52 turns、masking は 44 turns（+15%）。Qwen3-Coder 480B では summary が Raw 比 +15%、masking 比 +13% — [2508.21433](https://arxiv.org/abs/2508.21433)。圧縮後に blocked action と repeated exploration が増え、run 間の信頼性も下がる（AppWorld 147 tasks、MiniMax-M3）— [2608.06503](https://arxiv.org/abs/2608.06503)。
- **タイミング自体の害**: 閾値はモデルが導出の途中か否かを区別できず、不適切な時点では必要な途中結果を捨てる — [2606.23525](https://arxiv.org/abs/2606.23525)（著者の主張。対照実験は上表の範囲）。

#### 遅すぎる場合
- **窓枯渇**: window 端に近い閾値は「防ぐはずの失敗の後に発火する」。48K 閾値は 4/50 sessions でしか発火しなかった。reasoning model で狭い窓なら input budget の半分程度が機能し、背景実行では 1 turn 分の余地を残す必要がある — [2610.25516](https://arxiv.org/abs/2610.25516)。ただしこの study 単体は有意差なしで、著者も独立の evidence ではないとしている。
- **cache read の累積**: per-token では安くても、長い session では全過去トークンを毎回読むため最大の費用項目になる。削減分は cache read の減少によるものと報告 — [2609.26779](https://arxiv.org/abs/2609.26779)。
- **長文脈の品質劣化**: docs は「conversation の成長で応答品質が低下する」と述べるが定量値はない — [Anthropic compaction overview](https://platform.claude.com/docs/en/build-with-claude/compaction)。2610.31778 は遅い compaction のリスクを「task description と初期決定の過少利用」と整理するが、自身では検証していない — [2610.31778](https://arxiv.org/abs/2610.31778)。
- **latency**: τ が高いと文脈は保たれるが latency が伸びる（理論的 tradeoff の記述）— [2605.23296](https://arxiv.org/abs/2605.23296)。

### Inferences
- 対象システムの turn-end 方式は、Bud の「閾値を窓端に置くと failure の後に発火する」問題を部分的に回避する。ただし in-turn 発火の閾値は別に残るため、1 turn が 97K（870K→967K の差）を超えて伸びる場合は engine 側の compaction が走る。
- 回数に比例する損失（Q3）とタイミングの損失は別物である。閾値を下げると per-request 費用は減るが compaction 回数が増え、制約の喪失が累積する。「早めにすれば品質も良くなる」とは言えない。

### Gaps
- 再読込によって増えるトークン量を閾値別に測った研究は見つからなかった。
- 早すぎるコストと遅すぎるコストを同一実験で金額換算した、1M 窓のデータは見つからなかった。

## 3. 1 回の compaction でどれだけ情報が失われ、繰り返すとどうなるか

### Takeaway
損失は内容の種類で大きく異なる。成果物に関する情報は復元しやすく、行動制約は失われやすい。制約の喪失は回数とともに累積する点で複数研究が一致するが、モデル差は大きく、benchmark の成功率には表れにくい。

### Cited Findings

#### 1 回あたり
- **実 session の制約**: Claude Code の 44 sessions、269 制約で 81% 保持、19% 喪失（別系統の judge では 24.5%）。喪失率は feature requirement 6.2%、process instruction 35.9%、scope exclusion 36.8% — [2610.31778](https://arxiv.org/abs/2610.31778)。
- **事実**: 2,000 facts を 36.7× 圧縮すると 40% 正答、60% は不明回答、捏造 0% — [2603.17781](https://arxiv.org/abs/2603.17781)（シミュレーション）。
- **set membership**: Opus 4.8 が 15,000 URLs を約 14 kilobits に要約すると誤り率 0.505/0.535/0.555 でランダム相当、同サイズの Bloom filter は約 1/3 — [2608.01326](https://arxiv.org/abs/2608.01326)（要約に最も不利な設計である点に注意）。
- **safety rule**: type-blind な方式の 1 回の recall は最良でも budget 50%/25%/10% で 0.53/0.39/0.24（50 configs、622 constraints）。hierarchical truncation の保持は 50% — [2608.22752](https://arxiv.org/abs/2608.22752)。
- **要約 budget**: 300→15 words で constraint 残存 88%→23%、違反 7%→28% — [2606.22528](https://arxiv.org/abs/2606.22528)。
- **反証的な結果**: Bud では 134 回の compaction すべてで critical literal recall 1.000、圧縮は平均 24.7K→11.4K（−54%）。ただし検査対象は literal に限られる — [2610.25516](https://arxiv.org/abs/2610.25516)。

#### 反復
- **ProgramBench（3 種の anti-cheating rule）**: 喪失カテゴリ数は 1 回目 0.92、10 回目 1.93、15 回目 2.41、30 回目 2.65。モデル別の 1 回あたりは GPT-5.3-Codex 2.73、GPT-5.4 1.76、Opus 4.6 0.51 — [2610.31778](https://arxiv.org/abs/2610.31778)。
- **20 constraints の模擬会話**: 1/2/3 回で保持 91%/62%/46%（累積 9×/17×/31×、5 seeds）— [2603.17781](https://arxiv.org/abs/2603.17781)。
- **Claude Code の /compact on Sonnet 4.6**: safety rule の保持は 1 回で 53%、5 回で 10%（20 configs、1 回 50% 圧縮）— [2608.22752](https://arxiv.org/abs/2608.22752)。
- **違反率**: 0%→30%（最大 59%）、R=0→4 回で 0%→78%（7 モデル、1,323 episodes）。constraint が残れば違反 0%、消えれば 38% — [2606.22528](https://arxiv.org/abs/2606.22528)。
- **制御制約の保存**: 保持すれば loss of control 0%、省くと 87% — [2609.11024](https://arxiv.org/abs/2609.11024)。
- **性能への影響はモデル依存**: Opus 4.6 と GPT-5.4 は compaction 回数と accuracy に関係なし（GPT-5.4 は 16 回以上でも劣化なし）。GPT-5.3-Codex は約 8 回から低下し、16 回以上で平均比約 −0.10 — [2610.31778](https://arxiv.org/abs/2610.31778)。
- **理論上の予測**: 不可逆な要約では誤差が回数に対し super-linear に増えるとの予測がある。これは survey の prediction であり測定値ではない — [2607.08032](https://arxiv.org/abs/2607.08032)。

#### 緩和策の evidence
- 再要約を避ける（compaction を再度 compact しない）— [2609.26779](https://arxiv.org/abs/2609.26779)。
- 制約の pinning は約 47 tokens で違反を 0% に戻す — [2606.22528](https://arxiv.org/abs/2606.22528)。
- 直近履歴を保持（SS-p vs SS）で +6.7/+8.7/+8.0 pp、要約前の tool output 消去（TRC+SS vs SS）で +7.7/+9.7/+9.7 pp（SWE-bench）— [2609.32961](https://arxiv.org/abs/2609.32961)。
- 2610.31778 は「前の文脈をファイルに保存する」方式でも default を超えないと報告し、benchmark が復元可能な設計である点を compaction-friendly と呼ぶ — [2610.31778](https://arxiv.org/abs/2610.31778)。

### Inferences
- 閾値を 870K→167K に下げると、同じ session 長で compaction 回数は概算 5〜6 倍になる（差分 850K vs 147K、要約後 20K と仮定）。累積する喪失の観点では不利である。緩和策なしに閾値だけ下げるのは品質面で中立ではない。
- CLAUDE.md などに置いた制約は compaction 後に再読込されるので（[Claude Code context window](https://code.claude.com/docs/en/context-window.md)）、低閾値運用の補償として、会話中の指示を永続先へ移す仕組みが効くと考えられる。これを閾値別に検証した研究は見つからなかった。
- benchmark の成功率は制約喪失に鈍感である。閾値変更の評価を成功率だけで行うと、この種の損失を見落とす。

### Gaps
- 喪失した制約が実 session の最終成果に与える影響は未測定である（2610.31778 も同様に述べる）。
- 閾値そのものを変えたときの 1 回あたり喪失率（大きい文脈を一度に要約する方が多く失うのか）は確認できなかった。
- 上記の割合は benchmark と judge の違いにより相互比較できない。

## 4. production agent の閾値と、提示されている根拠（すべて practice であり evidence ではない）

### Takeaway
閾値は窓の 50% から窓のほぼ上限まで広がる。調べた範囲で、自社既定値の根拠となる比較実験を公開している vendor はなかった。

### Cited Findings

| 製品 | 既定の trigger | 確認元 | vendor が示す根拠 |
|---|---|---|---|
| Claude Code | native 1M 窓のモデルは約 967K。`autoCompactWindow` は 100000〜1000000、既定は unset（モデルに調整された値）。`CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` は下げる方向のみ。idle compaction は 200,000 tokens 以上かつ約 1 時間無操作 | [model-config](https://code.claude.com/docs/en/model-config.md)、[settings-reference](https://code.claude.com/docs/en/settings-reference.md)、[env-vars](https://code.claude.com/docs/en/env-vars.md) | なし。natural break での手動実行を推奨 |
| Claude API（threshold compaction） | 既定 150,000 input tokens、最小 50,000 | [compaction-threshold](https://platform.claude.com/docs/en/build-with-claude/compaction-threshold) | なし。docs は on-demand compaction を優先するよう案内 |
| Anthropic cookbook | 5k〜20k、50k〜100k、100k〜150k の目安、既定 100k。単発 notebook 実行で input 204,416→82,171 | [cookbook](https://platform.claude.com/cookbook/tool-use-automatic-context-compaction) | 1 回のみの実行で品質は未測定 |
| OpenAI Responses API | `compact_threshold` の既定は未記載、例はすべて 200,000 | [OpenAI compaction](https://developers.openai.com/api/docs/guides/compaction.md) | なし |
| Codex CLI | `auto_compact_token_limit` = min(設定値, 窓×9/10)。`effective_context_window_percent` 既定 95。`model_post_turn_compact_threshold_percent` は未設定で 0（turn-end compaction 無効）。`async_compaction` feature 有効時の閾値既定は 65%（config で変更可） | [openai_models.rs](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/openai_models.rs)、[config/mod.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs)（2026-11-05 時点の main） | なし（コード上の値のみ） |
| Gemini CLI | `model.compressionThreshold` 既定 0.5。直近 30% を保持 | [Gemini CLI configuration](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md)、保持率は [2609.00006](https://arxiv.org/abs/2609.00006) | なし |
| Aider | `max_chat_history_tokens` = min(max(max_input_tokens/16, 1024), 8192)。超過で要約 | [models.py](https://github.com/Aider-AI/aider/blob/master/aider/models.py)、[history.py](https://github.com/Aider-AI/aider/blob/master/aider/history.py) | なし |
| OpenHands SDK | `LLMSummarizingCondenser`: `max_size` 240 events、`max_tokens` は既定 None、`keep_first` 2。agent LLM の入力上限も考慮 | [llm_summarizing_condenser.py](https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/context/condenser/llm_summarizing_condenser.py) | なし |
| Hermes | 窓の 50% − max-tokens、下限 64K | [2609.00006](https://arxiv.org/abs/2609.00006) のソース調査（二次） | なし |
| Pi | 窓 − 16,384、直近 20K を保持 | 同上（二次） | なし |
| OpenCode | `limit.input` − 出力予約、chars/4 推定 | 同上（二次） | なし |
| Cursor | 「context limit で自動要約」までを確認。staff 投稿は窓超過時に古いメッセージを要約すると説明 | [Cursor forum（staff, 2025-06）](https://forum.cursor.com/t/summarizing-chat-context-why/102842/2) | なし |

- 2609.00006 は 11 システム中 7 つが threshold-triggered LLM compaction に収束していると報告し、「de facto standard」と呼ぶ — [2609.00006](https://arxiv.org/abs/2609.00006)。これは収束の観察であり、優位性の evidence ではない。
- Anthropic の engineering 記事は compaction を「context window の限界に近づいた会話」に行うものと説明し、数値の trigger を示さない。調整助言は recall 優先で prompt を調整することである — [Effective context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)。

### Inferences
- Codex の `model_post_turn_compact_threshold_percent` は、対象の mod が行う turn-end compaction と同じ発想の設定である（ただし既定は無効）。

### Gaps
- **Claude Code の buffer は未解決の不一致**: 2609.00006 は `AUTOCOMPACT_BUFFER_TOKENS = 13 000`、post-compact の予算 50,000 と記す。docs は約 967K とだけ述べる。依頼文の 1M→約 967K、200K→約 167K は約 33K の offset を意味するが、これを説明する一次記述は確認できなかった。また 167K という値も今回読んだ docs の範囲では確認していない。
- **Cursor の約 90%**: 検索要約は staff コメントが出典とするが、取得した staff 投稿の該当箇所では確認できなかった。未確認として扱う。
- **Codex の blog 値（80%、85〜90%、v0.100.0 以降 90% clamp、旧 issue の 220k）**: 非公式情報または古い情報である。現行コードの値を優先した。
- **OpenCode のソース**は取得できなかった（raw URL が 404）。Hermes、Pi、OpenCode の値は 2609.00006 のみに依拠する。
- Claude Code の「モデルに調整された窓」の算出根拠は未公開である。

## 5. prompt cache の経済性が trigger タイミングに意味すること

### Takeaway
cache read が 0.05× まで安くなっても、870K を毎 request 読む費用は 167K の約 5 倍である。単純な費用モデルでは、費用最小の閾値は 870K より下（想定条件で約 60K〜400K）に出る。ただし cache の warm 状態での compaction は安価で、品質は費用モデルに含まれない。費用だけを根拠に閾値を決めるべきではない。

### Cited Findings
- **Opus 5.5 の価格**: input $4、5m write $5、1h write $8、cache read $0.20（0.05×）、output $20（/MTok）。Sonnet 5.5 は $2、$2.50、$4、$0.10、$10 — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)。
- **長文脈**: Claude 4.6 以降（Haiku 5.5 除く）は 1M 窓まで標準単価で、900k の request も 9k と同じ単価。Haiku 5.5 は 100,000 tokens 超で高単価 — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)。
- **TTL**: API key は main conversation も 5 分既定、subscription 内は main が 1 時間。compaction は「それ以外」の区分 — [Claude Code prompt caching](https://code.claude.com/docs/en/prompt-caching.md)。cache 寿命は request 開始時点から計測し、使用で無料更新される — [Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)。
- **compaction と cache**: 要約 request は親と同じ prefix を使うので warm なら cache から読み、「文脈サイズから想像される費用の一部」で済む。cold では履歴全体を uncached で再処理する。compaction 後の turn は短い summary 分だけ cache を再構築する。conversation 層は無効化され、system prompt 層は残る — [Claude Code prompt caching](https://code.claude.com/docs/en/prompt-caching.md)。parent と同一 prefix で要約する理由と compaction buffer の必要性は engineering blog に説明がある — [Prompt caching is everything](https://claude.dev/blog/lessons-from-building-claude-code-prompt-caching-is-everything/)。
- **課金**: threshold compaction は追加の sampling step として billing と rate limit に計上され、`usage.iterations` の合計で把握する。過去の compaction block の再適用は無料 — [compaction-threshold](https://platform.claude.com/docs/en/build-with-claude/compaction-threshold)。
- **研究側の裏付け**: 費用は cached 単価の適用範囲と呼び出し回数で決まり、トークン数に比例しない — [2609.32961](https://arxiv.org/abs/2609.32961)。上限を低く固定すると prefix が安定して −20〜70% という報告がある一方、同じ著者は eviction 頻度が高いと caching が純損失になりうると述べる — [2606.11213](https://arxiv.org/abs/2606.11213)。

### Inferences
以下はすべて確認済みの Opus 5.5 価格に、筆者の仮定を組み合わせた試算である。測定値ではない。

**1 回あたりの費用**（S=要約サイズ。warm の compaction は read×C + (output+5m write)×S、cold は input×C + 同）

| 文脈 C | S（仮定） | read 費用/request | warm compaction | cold compaction | 回収 request 数 | TTL 切れ時の全再書込（5m / 1h） |
|---|---|---|---|---|---|---|
| 167K | 10K | $0.033 | $0.28 | $0.92 | 約 9 | $0.84 / $1.34 |
| 167K | 20K | $0.033 | $0.53 | $1.17 | 約 18 | $0.84 / $1.34 |
| 870K | 20K | $0.174 | $0.67 | $3.98 | 約 4 | $4.35 / $6.96 |
| 870K | 50K | $0.174 | $1.42 | $4.73 | 約 9 | $4.35 / $6.96 |
| 967K | 20K | $0.193 | $0.69 | $4.37 | 約 4 | $4.84 / $7.74 |

**費用最小の閾値**: 文脈が 1 request あたり g 増え、S まで圧縮を繰り返す鋸歯状モデルでは、1 request あたりの費用は次の式で表せる。在庫理論の EOQ と同じ形である。

`f(T) = r(S+T)/2 + w·g + (r·T + (o+w)·S)·g/(T−S)`、`T* = S + sqrt(2·g·S·(1+(o+w)/r))`

Opus 5.5 と Sonnet 5.5 はともに (o+w)/r = 125 なので T* は同じである。

| S | g（tokens/request） | T* | f(T*) | f(167K) | f(870K) | f(967K) |
|---|---|---|---|---|---|---|
| 10K | 1K | 約 60K | $0.017 | $0.025 | $0.094 | $0.103 |
| 20K | 3K | 約 143K | $0.044 | $0.045 | $0.106 | $0.116 |
| 30K | 5K | 約 224K | $0.071 | $0.073 | $0.121 | $0.130 |
| 50K | 10K | 約 405K | $0.133 | $0.181 | $0.159 | $0.167 |

- 仮定: g と S は一定、cache は常に warm、5m write、通常 turn の output と再読込は無視。g と S の実測値は未取得であり、上記は説明用の値である。
- 読み取り: g が 5K 以下の 3 例では 870K は最適値の約 1.7〜5.4 倍の費用になり、167K は約 1.0〜1.4 倍に収まる。g=10K の例では 167K の方が 870K より高くなる。したがって固定閾値より g に応じた閾値に費用面の意味がある。
- **turn-end は安い瞬間である**: turn 終了直後は cache が warm であり、compaction の費用が最小になる。依頼文の mod の between-turn 方式は経済的に妥当である。
- **最大の tail risk は TTL 切れ**: 870K で 5 分以上空いて再開すると約 $4.35 の再書込になり、167K の約 5 倍である。API key 運用で離席の多い場合は、turn-end で閾値を下げる費用面の動機になる。Claude Code の idle compaction は 1 時間 TTL の session が対象である（[settings-reference](https://code.claude.com/docs/en/settings-reference.md)）ため、5 分 TTL の session はこの保護を受けない。
- 回収 request 数を見ると、870K の session が compaction 後に 4 request 以上続くなら warm compaction の費用は回収できる。session がその前に終わるなら compaction は損である。
- 費用面の優位は、Q3 の回数増による制約喪失と引き換えである。品質を金額換算する方法は見つからなかった。

### Gaps
- Claude の compaction の実際の summary サイズ S と、実 session の g の公開分布は見つからなかった。対象システムのログからの実測が必要である。
- subscription 利用時の usage 上限の消費と API 単価の関係は確認していない。金額の試算は API 課金に対するものである。
- 1M 窓で cache 込みの実費を閾値別に測った実験は見つからなかった。
- `autoCompactWindow` の最小値は docs 上 100000 だが、`CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` で窓の割合を下げられるため、T*≈60K の水準は設定上到達できると読める（実機では未確認）。
