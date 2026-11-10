# Claude Opus 5.5 公式ドキュメントから得た設計条件

調査日: 2026-11-05。ユーザーの指示により、設計は Claude Code で今使いやすい Opus 5.5 に最適化してよい。以下はベンダーの一次情報であり、査読研究ではない。「Anthropic's testing」の値は公開データ付きの比較ではなく、独立に検証していない。

## 引用した事実

### effort
- medium が Opus 5.5 の既定値。「Start at `medium`」「test several levels against your own evals」。 — [Prompting Claude Opus 5.5, Calibrate effort](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5)
- Anthropic のテストでは、Opus 5.5 の medium は coding と knowledge-work の評価で Opus 5 の high 以上。いくつかの coding 評価では low がそれに近い。level 名はモデル間で同じ思考量を意味しない。 — 同上
- 同じ level でも Opus 5.5 は Opus 5 よりターンあたりの思考が多く、特に xhigh と max で顕著。 — 同上
- 「Reserve `xhigh` and `max` for work where you've measured a quality gain.」 — 同上
- agentic coding の長いターンでは `max_tokens` 128,000(モデル上限)が Anthropic のテストで良好。thinking は `max_tokens` に含まれる。 — 同上
- xhigh の想定用途は「Long-running agentic and coding tasks (over 30 minutes) with token budgets in the millions」。 — [Effort, Effort levels](https://platform.claude.com/docs/en/build-with-claude/effort)
- 「Effort is a behavioral signal, not a strict token budget.」 — 同上
- tool-use loop で tool result を処理するだけの後続リクエストは、どの level でも thinking を省略しうる。 — [Effort, Effort with thinking](https://platform.claude.com/docs/en/build-with-claude/effort)
- 高 effort は tool 呼び出しの増加や説明の増加を招きうる。低 effort は tool 呼び出しをまとめて減らす傾向がある。 — [Effort, Effort with tool use](https://platform.claude.com/docs/en/build-with-claude/effort)
- Opus 5.5 では `thinking: disabled` と `budget_tokens` は 400 エラー。TALE や Snell など固定予算を操作した研究の条件は、API 上は再現できない。 — [What's new in Claude Opus 5.5](https://platform.claude.com/docs/en/models/opus-5-5/whats-new-opus-5-5)

### prompt cache と effort 変更
- 「Changing the top-level `effort` value between requests invalidates the prompt cache.」Opus 5.5 では per-message effort(beta)なら cache を保つ。 — [Prompting Claude Opus 5.5](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5)
- per-message effort は、新しい入力を持つ `user` ターンの直後に置くとその返答に効く。それ以外の位置では次の新しい入力から効く。**tool_result だけの `user` ターンは新しい入力に数えないので、tool loop の途中で置いた変更は次の新しい入力まで効かない。** — [Effort, Per-message effort](https://platform.claude.com/docs/en/build-with-claude/effort)
- ベストプラクティスは「Hold top-level effort constant within cached conversations」。 — [Effort, Best practices](https://platform.claude.com/docs/en/build-with-claude/effort)
- mod の README には、Claude Code の測定で effort 変更後も cache が保たれたとある。上記と両立するかは audit.md を参照。mod が turn.step で書き換えた effort を Claude Code がどの形で送るかは未確認。

### window と価格
- context window 1M、max output 128K。 — [Claude Opus 5.5 overview](https://platform.claude.com/docs/en/models/opus-5-5/overview)
- input $4、output $20、5 分 cache write $5、1 時間 cache write $8、cache read $0.20(いずれも MTok あたり)。 — 同上
- 計算値: リクエストごとの cache read は 870k で $0.174、167k で $0.033 で、compaction_trigger.md の値と一致する。

### compaction API(Claude Code の /compact と同じ実装かは未確認)
- threshold compaction の既定 trigger は `input_tokens` 150,000、最小値は 50,000。選び方の推奨はなく、例は 100,000。 — [Compaction threshold](https://platform.claude.com/docs/en/build-with-claude/compaction-threshold)
- on-demand compaction は完了したターンの後に実行でき、発動点は「a limit you choose, below the model's context window」。 — [Compaction on demand, When to compact](https://platform.claude.com/docs/en/build-with-claude/compaction-on-demand)
- 要約の呼び出しはリクエストの model、system、tools、thinking 設定、`max_tokens` を使う。`max_tokens` は thinking を含む呼び出し全体の上限なので「allow several thousand tokens」。 — 同上
- 要約範囲内の per-message effort は引き継がれない。 — 同上
- custom `instructions` は既定プロンプトを補足せず完全に置き換える。Claude 5.1 以降では、custom instructions を使うと要約入力は visible conversation のみで、それ以前の thinking blocks は含まれない。 — [Compaction threshold](https://platform.claude.com/docs/en/build-with-claude/compaction-threshold)
  - Claude Code の `/compact <focus>` と mod の `guidedCompaction` が同じ挙動かは未確認。
- ドキュメントは要約 effort の推奨値を示していない(読んだ範囲。threshold ページの末尾 29,010 文字は未読)。

### 長時間実行
- Opus 5.5 は long-running work と parallel subagents を想定し、途中で進捗を報告してターンを終えることがある。 — [Prompting Claude Opus 5.5, Unattended agentic runs](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5)
  - ターン終了時に compaction する mod では、これが発動機会を増やす可能性がある(推論)。

## 設計への含意(推論であり、ドキュメントの主張ではない)
1. medium を常用の基準にする現行の既定値は、ドキュメントと一致する。
2. コンテキスト量だけを理由に xhigh へ上げる動作は「測定した品質向上がある場合に限る」という指針と合わない。この環境での測定は存在しない。
3. max での compaction にも同じ指針が当てはまる。要約は thinking を返さず、上限に thinking を含むので、高 effort は出力枠の圧迫要因になりうる。
4. トップレベル effort の変更が cache を無効にするなら、ステップごとの変更は 1M 近いコンテキストほど高くつく。cache 維持が実測どおりかを確定させる必要がある。
5. API の既定値 150k は唯一のベンダー提示の絶対値だが、根拠は示されていない。研究の 160k〜380k の比較と合わせて検討する材料であり、単独で採用する根拠にはならない。

## 未確認
- Claude Code の `/compact` が使う API、要約時の effort と `max_tokens`。
- Claude Code が effort を per-message とトップレベルのどちらで送るか。
- Opus 5.5 での context 長別の性能データ。読んだドキュメントには見当たらず、system card は未読。
