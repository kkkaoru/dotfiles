# 独立監査: 動的 effort と compaction の横断調査

調査日は 2026-11-05。論文は arXiv HTML を新規に取得し、図は SVG / PNG を描画して目視で読んだ。作業場所は共有の `scratchpad/papers` ではなく `scratchpad/audit/indep.hwv4/` である。5 本のノートは編集していない。

**設計判断に影響する指摘**

1. **SelfCompact の +6.3 は別ベンチマークの値である。** +6.3 は BrowseComp の Mimo-V2-Flash で、並記の +8.5 / +9.2 / +5.3 は BrowseComp-Plus の値である。BrowseComp-Plus での対固定閾値の最大は +5.3 にとどまる。固定閾値と比べると、費用は 12 セル中 8 セルで SelfCompact の方が高い。
2. **Fig. 6 の「faithfulness は effort で下がる」は全データセットに当てはまらない。** 低下は主に low 以降で、ArXiv は minimal→low で上昇し、SciGen は下がらない。
3. **ACON の guideline 効果として書かれた peak 削減は、比較元が違う。** 9.93K は No compression の値である。Prompting の 6.93K と比べると、ACON の peak は増える。
4. **Opus 5.5 では、Claude Code での effort 変更は実測で cache を保っていた**(269 組、hit 率 0.5 未満の組なし)。全組で top-level と per-message block が同時に変わっている。記録された値を wire の値と見れば、docs の「always invalidates message blocks」は、この組み合わせには文字どおりには当てはまらない。top-level **のみ**の変更は観測できず(n=0)、未検証である。Sonnet 5.5 の変更例は手元に 0 件である。
5. **未解決の最重要点として、新しい effort が変更時点の request で効いているかは確認できていない。** docs によれば、tool_result のみのターンの後に置いた per-message 変更は次の新規入力まで効かない。実際に効かないと言える証拠はなく、wire 上の位置は 279 件中 43 件しか判定できなかった(下記 7 章)。cache の維持は効果の証拠にならない。
6. **ARES の 42.0 vs 38.0 は約 50 タスク中の 2 タスク差である**(算出)。CI の記載は見当たらなかった。

判定は 4 区分である: **確認**(confirmed)、**行・条件違い**(wrong row or condition)、**値違い**(wrong value)、**未発見**(not found)。

## 1. summary_effort.md

### 2512.03503(Understanding LLM Reasoning)

HTML では caption が表の**後ろ**に置かれている。caption の直後の行を読むと表を 1 つ取り違えるので注意が必要である。下記は表番号を確定したうえでの判定で、Tables 9, 11 との再掲一致でも確かめた。

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| GPT-5 の think ability を上げると faithfulness が「consistently declines」、AlignScore の低下が最大、ROUGE / BERTScore は横ばい | Sec. 5.3 VII、Fig. 6 | **行・条件違い** | 引用は原文どおりだが、図とは完全には一致しない。対象は 3 データセットのみ。AlignScore(minimal / low / medium / high、目視): ArXiv 0.915 / 0.952 / 0.77 / 0.635、WikiHow 0.985 / 0.964 / 0.85 / 0.735、SciGen 約 0.987 / 0.987 / 0.99 / 0.992。SummaC: ArXiv 0.455 / 0.470 / 0.442 / 0.425、WikiHow 0.384 / 0.381 / 0.382 / 0.364、SciGen 0.266 / 0.272 / 0.281 / 0.280。SciGen は下がらず、minimal→low の低下も一貫しない。low を既定にする判断と矛盾しないが、「effort を上げるほど単調に悪化」とは読めない。ROUGE、BERTScore はほぼ横ばいで確認 |
| 「Fig. 6 の effort 別の数値は図のみ」「Fig. 6 専用の n は明記なし」 | Sec. 5.3、Fig. 6 | 確認 | 図から上記の概数は読める |
| 8 データセット平均の AlignScore: Vanilla 72.18、o1 65.70、o3 69.63、GPT-5 72.86。LRM は medium 固定 | Table 5、Table 11、Sec. 4 | 確認 | `[行未照合]` は解除できる |
| ROUGE: Vanilla 19.24、GPT-5 16.99 | Table 3、Table 9 | 確認 | `[行未照合]` は解除できる |
| 人手 faithfulness: GPT-5 4.42、SC 4.27、Vanilla 4.22、Deco 4.22、E2A 3.98、博士学生 2 名 | Table 7 の Average / Fai 列、Appendix I | 確認 | `[行未照合]` は解除できる |
| SC / IR の AlignScore 60.03 / 60.13、r = −0.685、p = 0.014 | Table 5、Fig. 3 | 確認 | |
| G-Eval 4.96–4.99、人手 3.98–4.42 | Sec. 5 V | 確認 | |
| Table 6 の COT 行、o1 行の不整合(ノートでは未検証) | Tables 5, 6 | 確認 | COT 行は Table 5 と Table 6 で全セル同一。o1 行も一部のセル(27.41 / 86.62 / 27.50 / 49.92 / 34.06 / 94.17)が COT と同じ。さらに medium の AlignScore は Fig. 6 と Table 5 のデータセット別の値が食い違う(ArXiv 図 約 0.77、表 56.30。WikiHow 図 約 0.85、表 42.34)。平均列はデータセット別の値から再計算でき(Vanilla 72.18)、表同士は整合する。Fig. 6 は別の実行の可能性もあり、どちらが正しいかは出典からは分からない |

### 2610.31778(Compaction-Friendly)

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| rule 喪失 Opus 4.6 0.51、GPT-5.4 1.76、GPT-5.3-Codex 2.73 を「Table 1 と Sec. 4.2」に帰属 | Sec. 4.2、Table 3(Appendix 6.9)。effort は Table 1 | **行・条件違い**(出典のみ) | 値は正しい。rule 喪失の表は **Table 3** で、Table 1 は effort(max / medium / medium)のみ |
| 回数別 0.92 / 1.93 / 2.41 / 2.65 | Sec. 4.2、Fig. 4(a) | 確認 | 全モデルをプールした値。caption は 28 回目以降を信頼性が低いとしており、2.65(30 回目)はその範囲にある |
| ProgramBench の rule 喪失の n「2 タスク」 | Fig. 4(a) caption、Appendix 6.9 | **行・条件違い** | **3 タスク**である。erosion 分析には lua/lua が追加されている。2 タスクは accuracy 分析の方 |
| Haiku 4.5 要約: GPT-5.4 0.59→0.43、Codex 0.38→0.18、Opus は CI 内。3 論文、2 run / 設定 | Sec. 4.1、Sec. 3.1-3.2 | 確認 | 閾値 160K 固定の比較 |
| reasoning block 削除: GPT-5.4 0.59→0.43、Opus と Codex は影響なし | Sec. 4.1、Sec. 3.3 | 確認 | |
| SWE-chat: 44 セッション、269 constraints、81 / 19%、別 judge 24.5%、6.2 / 35.9 / 36.8% | Sec. 4.2 Q4 | 確認 | カテゴリ対応の `[抽出のみ]` は解除できる |

### 2606.22528v2(Governance Decay)

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| 違反率 0→30%、最大 59%、1,323 episodes、27 / cell | Table 2 | 確認 | |
| summarizer 別: GLM 7–13%、Claude 47–60%、DeepSeek 33–93%、n = 15 / cell | Table 3 | 確認 | 行が summarizer、列が agent。GLM agent × DeepSeek summary の 53% も確認 |
| budget 300→15 語: 生存 88→23%、違反 7→28%、DeepSeek 20→65% | Sec. 5、Fig. 3 | 確認 | n は本文に記載なし |
| pinning: タスク「同上」(soft 5 タスク)、n 未抽出 | Table 2、Sec. 7 | **行・条件違い** | 0% の根拠は **9 タスク、27 / cell、7 モデル**(攻撃あり / なし)。約 47 tokens は確認 |
| operator 詐称で 0→17%、provenance 付きで 10% | Sec. 7 | 確認 | 3 モデル、soft 5 タスク、n = 60 / cell。`[抽出のみ]` は解除できる |
| 保持 0%(n=90)、脱落 38%(n=315) | Sec. 5 | 確認 | 元の 5 モデルの grid、非 defense の 3 条件に限る |
| soft +50、hard +6、8.3× | Sec. 5、Table 1 | 確認 | |

### 2608.11242v2(Lost in Context Compaction)

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| extractor 90.3 / 93.7 / 95.1%、平均 93.0%、52.7 pp 差 | Table 4、Appendix C.3、F.4 | 確認 | 順に WildChat / Hermes / OpenResearcher。52.7 pp は WildChat の 90.3 と 37.6 の差。compactor 間の平均値 |
| GPT-5.4-mini: Anthropic→Pi 53.7→88.4、26.0→98.0、28.3→6.7、N=300(Hermes 285)、220K | Table 8 脚注 † | 確認 | 順に Hermes / OpenResearcher / WildChat |
| targeted prompt: retention 1.3→24.8、3.3→37.6、compliance 54.6→65.5、56.2→71.2 | Table 10 | 確認 | WildChat のみ。baseline は Table 8 の N=750 の行と一致 |
| 注入 SC の平均保持 17%、inbox 30/100、20 seeds × 5 message、圧縮比 182× | Abstract、Table 1、Appendix D | 確認 | |

### 2510.00615(ACON)

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| accuracy: Prompting 43.5→ACON 51.2 / 56.5、No compression 56.0、168 タスク | Table 1 | 確認 | |
| guideline の効果として「peak 9.93K→7.33K」 | Table 1 | **行・条件違い** | 9.93K は **No compression** の値。Prompting の peak は **6.93K** で、ACON は 7.17K / 7.33K と**増える**。guideline の効果は accuracy の向上で、peak 削減ではない |
| 最適化 model: o3 51.2、gpt-5 50.6、gpt-4.1 47.6 | Table 3 | 確認 | UT ステップのみの値 |
| compressor 費用 $0.045 / $0.014 / $0.0004 を「Table 4」に帰属 | Sec. 4.5 本文 | **行・条件違い**(出典のみ) | 値は正しい(−69.2%、−99.1%)が Table 4 にはない |
| end-to-end $0.331→$0.285、latency 73.24 s→87.68 s を同条件のように併記 | Table 4 caption | **行・条件違い** | 費用は gpt-4.1 agent + Qwen3-14B compressor(input caching 除外)。latency は Qwen3-14B agent + compressor(A100 でローカル実行)。別構成なので組み合わせて読めない |
| peak 26–54%、小型 LM 20–46%、8-objective QA 54.5%、最適化 $2 未満、rollout 約 $20 | Sec. 1、Sec. 4.2、Sec. 4.5 | 確認 | 54.5% と no compression 超えが同時に成り立つのは UT 行のみ(EM 0.373、F1 0.494)。UT+CO は EM 0.335 で下回る |

## 2. compaction_trigger.md

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| 2610.31778 の 160K〜380K sweep、1M 窓の 16〜38%、400K 窓の 40〜95% | Sec. 4.1 | 確認 | 論文自身が「scaffold 既定の近くは試していない」と明記する。GPT-5.4 の窓は Table 1 で 1,050K |
| GPT-5.3-Codex: 160K 0.38→260K 以上で約 0.60 | Sec. 4.1、Fig. 2 | 確認 | PaperBench のみ、valid submission 条件付き、3 論文 × 2 run。160K の compaction 回数は 27.71 ± 17.00(Table 2)。著者は頻度だけでは説明できないとしている |
| 1M の 2 モデルは閾値で差なし | Sec. 4.1 | 確認 | 1 閾値あたり少数 run なので、「差がない」ではなく「検出されない」と読むべきである |
| SelfCompact: 対 baseline +8.5 / +9.2 / +5.3 | Table 4、Sec. 4.2 | 確認 | BrowseComp-Plus の値 |
| SelfCompact: 対固定 30% は最大 +6.3 点 | Table 4、Sec. 4.2 | **行・条件違い** | +6.3 は **BrowseComp の Mimo-V2-Flash**(57.9 − 51.6)。BrowseComp-Plus では +4.1 / +5.3 / +2.7 で**最大 +5.3**。同じ行に並べると同一条件に読める。Overall は +4.9 / +4.6 / +4.5 |
| 費用 −67 / −63 / −33% | Table 4 | 確認 | 比較対象は compact なし。**固定閾値と比べると 12 セル中 8 セルで高く**、同等 1、安い 3(BrowseComp-Plus は 0.04 vs 0.04、0.07 vs 0.04、0.16 vs 0.14)。全 prompt を cache 単価で計算する近似である |
| SelfCompact が最強(論文の記述) | Table 4 | 確認(論文内の不整合あり) | BrowseComp-Plus の Mimo では Keep-last-N 63.5 が SelfCompact 62.9 を上回り、本文の「strongest for all three」と食い違う。CI は重なる |
| 最難 2 bin で 5〜20 pp、1 benchmark 150 問 | Sec. 4.2、Fig. 3、Appendix B | 確認 | |
| read 費用 870K $0.174、167K $0.033 | [pricing](https://platform.claude.com/docs/en/about-claude/pricing) の Opus 5.5 行 | 確認 | cache hit **$0.20 / MTok**(脚注 2、基本入力の 0.05×)で再計算すると $0.1740、$0.0334。ページ内の一般文「10%」は Opus 5.5 には当たらない。Sonnet 5.5 は $0.10 で半額 |
| 同表の warm / cold / TTL 再書込 / 回収 request 数、(o+w)/r = 125 | 同上 | 確認 | input $4、5m $5、1h $8、output $20 で全行を再計算し一致。subscription の main は 1h 書込になる点のみ前提として残る |

## 3. effort_context_interaction.md

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| OOLONG: GPT-5-nano の high / low、8K〜256K、64k 以降は差がほぼなく、256k で high がわずかに下 | Sec. 4.1、Fig. 3(a) | 確認 | 数値表なし。Oolong-synth の図のみ |
| OOLONG: real では差がごく小さい、400 問 / 長さ | Appendix B.1、Sec. 2.2 | 確認 | 「400 test split questions per context length」 |
| CL-bench Fig. 16 の 9 モデル、0-4K と 32K+ の全値 | Appendix D、Fig. 16 | 確認 | 図を描画して全 36 値を照合、すべて一致 |
| 再計算: 平均差 +1.5→+1.4、拡大 4 / 縮小 4 / 不変 1、相対比で 5 モデル拡大 | 同上 | 確認 | 再計算で一致(+1.51→+1.39)。全 bin の平均差は +1.51 / +1.82 / +1.13 / +1.84 / +1.39 で、**中間 bin を含めても拡大傾向はない**。API effort 3 モデルの平均は +0.5→+2.0 だが、GPT 5.1 を除くと +0.3→−0.75 になり、GPT 5.1 依存も確認 |
| 著者の「高 effort の優位は長いほど顕著」 | Appendix D | 確認 | 記述はあるが、著者は検定を示していない |
| GPT-5.1 全体 21.2→23.7% | Fig. 6 caption | 確認 | |
| Reasoning Shift の 74% = Gemini 3 Flash Preview、GPQA、Multi-turn、12,927→3,334 | Table 1、Sec. 3.2 | 確認 | 74.2% 減。**Long Input ではない**点もノートは正しく書いている。Long Input の最大は GPQA gpt-oss-120b の 65.5%、IMO では 50.9%(いずれも算出) |
| Long Input IMO: Qwen 74.5→67.8、gpt-oss 73.8→64.0、Table 6 の比較 | Tables 1, 6 | 確認 | |
| max effort prompt で「tokens は baseline で 10〜15% 増」 | Sec. 5.1、Table 6 | **行・条件違い**(軽微) | 著者の平均的な記述だが、Qwen は 28,771→28,755 で増えていない。10〜15% に当たるのは Gemma +15%、gpt-oss +13.8% |
| ARES TAU Airline: Medium 42.0 / 98k、High 38.0 / 873k、RL 42.0 / 133k、SFT 36.0 / 678k、Low 32.0 / 12k | Tables 1, 2 | 確認 | 133k は Table 2 のみ。**T_total / T_task = 98k / 1961 から約 50 タスクと算出され、42.0 vs 38.0 は 21 vs 19 タスク**。CI の記載は見当たらない。「medium が high に勝つ」は強く読めない。モデルは gpt-oss-20b で Claude ではない |
| ARES の 52.7% は Retail RL、Airline は 84.8% 減 | Table 2、Sec. 1 | 確認 | |

## 4. context_size_degradation.md

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| GPT-5.4 の 512K-1M 36.6、全 8 帯 | OpenAI GPT-5.4 発表(r.jina.ai 経由)、Opus 4.7 card Fig. 8.7.2.A / B(p.195-196) | 確認 | card で 79.3、36.6 を目視確認。いずれも xhigh。1M は Codex では experimental と記載 |
| GPT-5.5 の 512K-1M 74.0、全 8 帯 | OpenAI GPT-5.5 発表(r.jina.ai 経由) | 確認 | 同じく xhigh なので 36.6 との比較は同 effort。第二の出典はなく、openai.com の直接確認もできていない |
| Qwen2.5-7B 26.1 vs 1M 版 94.9(standard、128k) | 2506.02921 Fig. 9 | 確認 | 拡大して目視。窓 1,010,000 は Table 6 で確認。131,072 の行は 7B-Instruct について未照合 |
| 同表 multihop_2、7B-Instruct(131K)、128k = **1.2** | 2506.02921 Fig. 9 | **値違い** | 正しくは **1.5**。1.2 は multihop_5 の 16k の値。表の他の値はすべて一致 |
| Xue: 8/10 vs 3/10、Fisher p=0.0698、10,991 / 90,537 / 299,140 字、coverage 92.1 / 93.8% | 2607.17937v2 Table 2、Sec. 5.1.1 | 確認 | 39.7〜89.2% は**失敗率**の Wilson CI。著者は探索的と明記する(5 run から 10 run に拡張し、Irrelevant は 0/5 から 3/10 に動いた)。p は 0.05 を超える |
| Sonnet 4.6 max 90.3→65.8、64k 90.6→65.1、<1M subset 77.8(29 問)/ 71.3(54 問) | Sonnet 4.6 card Table 2.16.A(p.30)と脚注 15 | 確認 | 帯は (128k, 256k] と (524k, 1024k]。ページは p.30-32 |
| 「同じ 1M 窓・同じ帯」で GPT-5.4 36.6、GPT-5.5 74.0、Opus 76.0、Sonnet 65.8 を並べる(ノート 100 行目) | 上記各出典 | **行・条件違い** | 帯の定義が違い(512K-1M と (524k, 1024k])、Sonnet の 65.8 は **API の 1M 上限を超える問題を含む内部設定の結果**である。API で再現できる subset は 77.8 だが 29 問のみ。同一条件の比較としては読めない |

## 5. failure_escalation.md

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| Escalation 85.0% / $0.27、GPT-4 89.6% / $1.93、Retry 92.0% / $2.51、Reflexion 87.8% / $3.90 | 2407.01502 Table A1 | 確認 | 費用は 164 問の総コスト、5 run 平均。Table A2(2023 年 6 月版モデル)の 87.6% / $0.42 とは別物 |
| Handoff Tax: Raw 69.2% / $1.61、HC 79.2% / $0.72、LC 60.7% / $0.40、GPT 側の値、Abort $0.90 | 2608.24358 Table 1(a)、Sec. 4.1 | 確認 | 値はすべて一致 |
| 同 n「500 タスク」 | 同 Sec. 3.2-3.4 | **行・条件違い** | Table 1 は **switched subset(全戦略で switch した task の交差)を p5〜p50 の 7 switch point で平均**した値で、500 タスク全体の平均ではない。「$1.61 vs $0.72」は「途中まで走った難しめの subset」での比較である |
| ToolBench-X: TTS +3.5–11.5、Hint +25.5–35.5 | 2606.25819 Fig. 4 と直後の本文(節番号は未特定) | 確認 | 5 モデル、200 タスク。TTS は 10 round 追加。**両者とも「失敗した」ことを実験側が与えた条件で**、Hint は注入した異常の内容を教える。失敗を自分で検知する運用より有利な設定である |
| Mehta: 5/18 (28%)→16/18 (89%)、差 61 pt、CI 29–78、base +9%、tool +29%、design +21%、abridged 1/6 vs 1/6 | 2607.02436v2 Abstract、Table 2、Results | 確認 | Opus 4.7、非ランダム化。著者も p 値を記述統計と位置づける |
| SWE-smith: 40.2%→192 (38.4%)、repeat 4/6 警告、6/8 resample、89%、25% 超 / 4% 未満 | 2504.21798 Sec. 4.2、§F.5 | 確認 | ノートのモデル欄は「SWE-agent-LM-32B、Claude 3.7」だが、介入の話は本文の文脈では LM 側である。§F.5 の冒頭は精読していない |

## 6. opus_5_5_guidance.md(追加範囲)

| 主張 | 出典箇所 | 判定 | 正しい値・補足 |
|---|---|---|---|
| 「Changing the top-level `effort` value between requests invalidates the prompt cache.」、per-message なら維持 | [Prompting Claude Opus 5.5](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5) Calibrate effort | 確認 | 原文どおり |
| tool_result のみの user ターンは新規入力に数えない | [Effort](https://platform.claude.com/docs/en/build-with-claude/effort) Per-message effort | 確認 | 下記 7 章の表の該当行を参照。mod に直接関わる |
| `max_tokens` 128,000、価格、$0.174 / $0.033 | 同ページ、pricing | 確認 | |
| 「Claude Code が effort を per-message とトップレベルのどちらで送るか」未確認 | 下記 7 章 | 解消 | **両方**送っている |
| 「`/compact` の API、要約時の effort と `max_tokens`」未確認 | 下記 7 章 | 一部解消 | 実測はできず、コードからの推定のみ |

## 対立の解決 1: effort 変更と prompt cache

**docs の記述(原文)**

- [Prompt caching の表](https://platform.claude.com/docs/en/build-with-claude/prompt-caching): 「Changing the `output_config.effort` value always invalidates message blocks, with the same model-specific effect on tool and system caches as thinking parameters. Setting effort explicitly to the model's default is equivalent to omitting it and does not invalidate. On models that support per-message effort, an effort change carried in a `role: "system"` message inside `messages` leaves the cached prefix intact.」
- [Effort](https://platform.claude.com/docs/en/build-with-claude/effort): 「On Claude Fable 5.1, Claude Mythos 5.1, Claude Opus 5.5, Claude Opus 5, Claude Sonnet 5.5, and Claude Haiku 5.5, use a per-message effort change, which keeps the prompt cache. On other models, set a new top-level value on the next request, which starts the cache over.」
- [Claude Code prompt caching](https://code.claude.com/docs/en/prompt-caching.md): 「On Opus 5.5, Sonnet 5.5, Haiku 5.5, and Fable 5.1 with an API key or a Claude subscription, you keep the cache when you change effort.」Bedrock、Google Cloud's Agent Platform、Claude apps gateway、`CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`、HIPAA 構成では「works as on most models」、つまり cache は保たれない。

**結論: 両立する。** API docs の無効化は top-level のみの変更についての記述で、同じ行に per-message の例外がある。Claude Code の docs は README の測定結果を、対象モデルと接続経路つきで明記している。README の「API documentation lists effort among cache-invalidating changes」は例外を省いた書き方である。README の「Bedrock unverified」は docs で答えが出ており、cache は保たれない。Foundry は docs の例外一覧に名前がなく、未確認のまま残る。

ノートの扱い: effort_context_interaction.md の 189 行目、198 行目は API 層の記述として正しいが、Claude Code 層の記述が欠けている。

## 対立の解決 2: autocompact buffer は文書化されているか

**docs の記述**: buffer の token 数は**文書化されていない**。`https://code.claude.com/docs/llms-full.txt` の全文検索で「Autocompact buffer」「33K」「167K」「13K」はいずれも該当しなかった。書かれているのは次の点のみである。

- [Default auto-compact thresholds](https://code.claude.com/docs/en/model-config.md): 「Models running with a native 1M window compact before the window fills, at about 967K tokens by default.」Fable 系、Sonnet 5 以降、Haiku 5.5、Opus 4.7 以降が対象。
- 同ページ: Sonnet 4.6 と Opus 4.6 の extended context なしは「compact at the 200K boundary」。それ以外の既定は「when the conversation reaches the model's context limit」。
- changelog: Sonnet 5 の 1M セッションは約 934K から約 967K に変更。別項目で effective context window は「reserves space for max output tokens」。env-vars: `CLAUDE_CODE_MAX_OUTPUT_TOKENS` を増やすと auto-compaction 前の effective context window が減る。

**コード上の値(文書ではない)**: ローカルの Claude Code 2.1.304 バイナリから次を確認した。

```js
function nQ(e,n){let r=e-13000, /* testPctOverride の処理 */ return r}
function WY(e,n){let r=Math.min(e.maxOutputTokens,Obr) /* Obr=20000 */ ... return h-r}
function CG(e,n){return nQ(WY(e,n),q_e(e,n))}
```

閾値 = 窓 − min(maxOutputTokens, 20000) − 13000 である。

**結論: 13K と 33K は矛盾しない。基準点が違うだけである。** 33K = 20K(出力予約)+ 13K(buffer)で、1M→967K、200K→167K となり、`/context` の実測と両窓で一致する。2609.00006 の「effective context window の下の buffer 13,000」も文言どおり正しい。compaction_trigger.md 157 行目の「未解決の不一致」は解消できる。

限界は 3 点ある。

- 確認したのは 1 バージョンのみで、将来も保たれる保証はない。
- `/context` の表示値がこの式から計算される経路までは追っていない。一致は数値の一致である。
- precompute 系の別経路(`precomputeBufferFraction`、server flag)があるので、この結論は既定の trigger に限る。

なお docs は 200K を「boundary」と書き、167K とは書いていない。docs だけを根拠に 167K とは言えない。

## 7. Opus 5.5 の cache 実測(追加範囲)

**方法**: `~/.claude/projects` の transcript から usage とメタデータのみを読んだ(本文は出力していない)。effort は記録されている: `assistant.effort`、`assistant.perTurnEffort`、`api-request.params.output_config.effort`、`api-request-blob`(role system の `output_config.effort`)。main thread の連続 request のうち、同一モデル、compact_boundary をまたがない、間隔 5 分未満、直前の context 5,000 tokens 以上のものを `requestRef` で結合した。hit 率は B の `cache_read / (input + cache_read + cache_creation)` である。

| 条件(Opus 5.5) | 組数 | session | hit 中央値 | p10 | 最小 | <0.9 | <0.5 |
|---|---|---|---|---|---|---:|---:|
| top-level 同じ(対照) | 28,468 | 35 | 0.9965 | 0.9862 | 0.035 | 65 | 8 |
| top-level 変更 | 269 | 14 | 0.9949 | 0.9807 | 0.660 | 2 | 0 |
| top-level 変更、直前 ≥200K | 111 | 10 | 0.9966 | 0.9901 | 0.977 | 0 | 0 |
| top-level のみ変更(system block なし) | **0** | — | — | — | — | — | — |

| 問い | 判定 | 根拠 |
|---|---|---|
| Opus 5.5 で Claude Code の effort 変更は cache を保つか | **保つ(この環境で)** | 上表。変更群の hit 率は対照とほぼ同じ。直前 911K までの範囲を含む。全 269 組が v2.1.304、oauth の beta あり(subscription と推定)、2026-10-29〜11-04 |
| Sonnet 5.5 でも保つか | **判定不能** | 変更の組は 0 件(assistant.effort で全 request を見ても 0)。README の Sonnet 5.5 の測定は手元の transcript では再現できない。docs の記述のみが根拠 |
| top-level のみの変更で本当に無効化されるか | **判定不能** | n=0。top-level と block を同時に変えた場合は保たれた(上表)が、top-level 単独の場合は確認も反証もできない。確定には API への直接実験が必要で、本監査では実行していない |
| effort の送り方 | **両方** | 変更の全 269 組で、`params.output_config.effort` が新しい値に変わり、**かつ** tail に `{role:"system", output_config:{effort:新しい値}}` がある。バイナリ: `jfn` / `Mur` が履歴の各時点の level から system block を挿入し、`bJe(us,Ol,...)` が top-level を設定する。beta は `per-turn-control-2026-07-01`(コード上の名称 `per_message_effort`) |
| docs の per-message の形との違い | **要注意** | docs の例は top-level を `high` に固定し、system block だけ `low` にする。header も `mid-conversation-output-config-2026-07-01` である。Claude Code は top-level も変え、header 名も異なる。記録された `params` は wire の値と見てよさそうだが、通信の capture はしていない。server がなぜ両者の変更でも cache を保つのかは不明 |
| 新しい effort が実際に効いているか | **判定不能(最重要)** | docs: 「A `user` turn that holds only `tool_result` blocks doesn't count as new input, so a change placed after one in a tool-use loop waits for the next `user` turn with new input.」top-level の変更がこれを上書きするかは不明。**位置の判定**: transcript の user レコードでは 283 件中 273 件が tool_result のみの直後だが、これは wire の形ではない。request の tail で system block の直前を見ると、判定できたのは 279 件中 43 件のみで、tool_result のみ 25、tool_result + text 10、新規入力のみ 8。残り 236 件は直前の blob が記録になく不明。**thinking tokens の比較**(変更時の request の中央値): medium→high は 69→38(n=98)、high→medium は 81.5→76(n=96)、low→medium は 49.5→0(n=30)、medium→low は 0→73(n=29)。上げても変更時点で増える様子はないが、0 が多く tool loop の位置とも交絡するので、効かないとは結論できない。後続 request も見ていない。確定には同一入力で level を変える実験が必要 |
| 直前 context に対する read の比が約 0.5 の組 | 指標の問題 | 174 組(変更群 6)すべて、直前 request の `usage.iterations` が 2 以上。合算で直前の total が膨らんでいるだけで、cache miss ではない(cache_creation は 2,670〜4,012 tokens)。この比を指標にしないこと |
| `/compact` の effort | **実測不能、コードからは session effort** | fallback 経路に `effortValue:Ch(s)`、`Ch(e)=xy(e.permissionLayers)??Od(...)`。主経路の fork は `cacheSafeParams` から継承と見られるが、最後まで追っていない。README の実測(low 934、max 1,898 output tokens)と整合する。docs は thinking の継承のみ記述し(v2.1.198)、effort は記述なし |
| `/compact` の `max_tokens` | **実測不能、コードからの推定はモデル上限** | 2 経路とも `maxOutputTokensOverride` を渡さない。`_b=Math.min(eo?.maxTokensOverride‖Q.maxOutputTokensOverride‖mt.maxOutputTokens, mt.maxOutputTokens)` なので `mt.maxOutputTokens` に落ちる。main の記録が 128,000 なので Opus 5.5 では **128,000 と推定**。`eo.maxTokensOverride` の由来は未確認 |
| 実測できない理由 | 確認 | `skipTranscript:true`。transcript に `querySource` が compact の api-request は 0 件 |
| `/compact` の方式 | **おそらく compaction API とは別** | `querySource:"compact"` の fork で要約指示を送る方式(`promptMessages:[summaryRequest]`)。docs も「sends a separate request」と記す。主経路は `skipCacheWrite:true`、fallback は `enablePromptCaching:false`。`context_management` beta が compaction に使われていないことまでは確認していない |

## 着手しなかった主張

- summary_effort.md: 2507.02145(Table 2 ほか)、Chen 2412.21187、Inverse Scaling 2507.14417、Bud ContextCompact 2.0、marina PR #529、2609.11024、2603.17781、CompactionRL 2607.05378、RLM 2512.24601、Goal Drift 2505.02709、MAST 2503.13657、CliffCompaction 2609.26779。
- compaction_trigger.md: 2609.32961、AgentDiet 2509.23586、2606.11213、Parallel Context Compaction、Codex CLI の設定値、EOQ 式の f(T) の表。
- effort_context_interaction.md: PredicateLongBench、WebAgents、MATH-500 の数値、OOLONG の over-reasoning の記述。
- context_size_degradation.md: Opus 4.6 card の値、Lost in the Middle、arXiv:2605.23170、1M GA blog の引用。
- failure_escalation.md: 列挙された 5 件以外(TACIT-Switch、ToolMaze、Self-Healing、ThinkBrake、Cycle Detection、SWE-Effi、Reflexion の 91%、Handoff Tax の Traj-drop の QRec)。
- opus_5_5_guidance.md: compaction API の記述(31〜35 行)と 41 行以降の推論。

## 確認の限界

- OpenAI の数値は r.jina.ai 経由の描画である。
- 図の値は目視で、Fig. 6 は小数第 2〜3 位まで概数である。
- docs の一部は WebFetch の抽出を経ている。effort、pricing、Claude Code docs の全文は原文を確認し、prompt caching の表は抽出結果を引用した。
- 実測は 1 ユーザー、Claude Code 1 バージョン、約 1 週間、変更 14 session の観察である。effort 変更は mod か `/effort` か区別していない。
- hit 率は usage のトップレベルの値で計算した。advisor を使う request では iterations が合算されるので、最後の iteration だけの値とは多少異なる。監査中に追加された local_session_measurements.md の同じ指摘(iterations の合算)、effort の記録項目、圧縮要求が記録されない点は本監査と一致する。同ノートのそれ以外は監査していない。
