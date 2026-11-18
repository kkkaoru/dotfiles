#!/usr/bin/env python3
"""Tokens/s for PLaMo 3 Translate on a fixed sample set, so a serving change can be compared.

    scripts/bench-plamo-translate.py --port 18555            # measure a running server
    scripts/bench-plamo-translate.py --port 18555 --reps 5   # more rounds
    scripts/bench-plamo-translate.py --port 18555 --json     # machine-readable

A server answering on --port is benchmarked as is (scripts/setup-plamo-translate.sh starts
~/.local/bin/mlx_lm.server on 18555). Each sample is warmed up first, then timed; the report is
generated tokens ÷ wall time per request, plus the sha256 of every output so two configurations
can be checked for identical translations. Run it on an idle machine: another GPU/CPU-heavy job
halves these numbers.
"""

import argparse
import hashlib
import json
import sys
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor

# Fixed inputs: both directions, short and long, prose and markdown-ish (a table row and a list,
# where the model repeats structure). Never reorder or reword these — the numbers are only
# comparable against the same list.
SAMPLES = [
    ("en", "ja", "The meeting has been rescheduled to next Monday due to a scheduling conflict."),
    ("ja", "en", "本日は晴天なり。開発環境のセットアップとテストを実行してください。"),
    ("en", "ja", "In software engineering, speculative decoding accelerates autoregressive language model inference by using a faster draft mechanism to propose multiple candidate tokens and verifying them in parallel with the target model, guaranteeing mathematically identical outputs under greedy decoding."),
    ("ja", "en", "並列提案とAR検証を組み合わせる際、最もバグを生みやすいのがKVキャッシュのライフサイクル管理です。そこで、ドラフトが作った一時KVと、検証後に次のサイクルへ引き継ぐ確定KVの境界を整理しておきます。"),
    ("en", "ja", "Apple Silicon unified memory lets the GPU read quantized model weights directly, without PCIe transfers."),
    ("ja", "en", "設定ファイルを変更したあとは、テストを実行してカバレッジを確認してください。"),
    ("en", "ja", "| Name | Type | Default | Description |"),
    ("ja", "en", "- サーバーを起動する\n- ポート18555で待ち受ける\n- 停止するときはSIGTERMを送る"),
    ("en", "ja", "The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs."),
    ("ja", "en", "翻訳の品質を保ったまま、推論速度を改善する方法を検討しています。"),
    ("en", "ja", "Run `bun run check` before committing; it runs the linter, the formatter check and the tests with coverage."),
    ("ja", "en", "この関数は、与えられた文字列を指定された言語に翻訳し、失敗した場合は元の文字列を返します。"),
]

POLITE = "<|plamo:key|>form<|plamo:val|>polite"
STOP = ["<|plamo:tag|>", "<|plamo:bos|>"]


def prompt(text: str, lang: str) -> str:
    """The PFML3 prompt the mods send (tools/claude-plamo-translate/hooks/translate.ts)."""
    return (
        "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>"
        f"{text}<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>{lang}"
        f"{POLITE if lang == 'ja' else ''}<|plamo:msg|>"
    )


def complete(port: int, text: str, lang: str, max_tokens: int = 256) -> dict:
    body = json.dumps(
        {
            "prompt": prompt(text, lang),
            "max_tokens": max_tokens,
            "temperature": 0,
            "stop": STOP,
            # A repeated prompt would be answered from plamo-mlx-server's cache in a millisecond,
            # which measures the cache instead of the model.
            "cache": False,
        }
    ).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/completions",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        return json.loads(response.read())


def tokens(answer: dict) -> int:
    usage = answer.get("usage") or {}
    return int(usage.get("completion_tokens") or 0)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=18555)
    parser.add_argument("--reps", type=int, default=3, help="timed rounds (default 3)")
    parser.add_argument(
        "--concurrency",
        type=int,
        default=1,
        help="requests in flight (default 1); the mods run up to 4",
    )
    parser.add_argument("--json", action="store_true", help="print JSON instead of a table")
    parser.add_argument("--label", default="", help="name for this configuration")
    args = parser.parse_args()

    # Warm-up: Metal shaders compile on the first forwards, and the first request pays for them.
    for _ in range(2):
        for lang, _, text in [(s[0], s[1], s[2]) for s in SAMPLES[:2]]:
            complete(args.port, text, lang, max_tokens=32)

    per_sample: list[dict] = []
    outputs: list[str] = []
    round_totals: list[tuple[int, float]] = []
    for _ in range(args.reps):
        started = time.perf_counter()
        with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
            answers = list(
                pool.map(
                    lambda sample: complete(args.port, sample[2], sample[1]),
                    SAMPLES,
                )
            )
        elapsed = time.perf_counter() - started
        total_tokens = sum(tokens(answer) for answer in answers)
        total_seconds = elapsed
        for sample, answer in zip(SAMPLES, answers):
            if len(outputs) < len(SAMPLES):
                outputs.append((answer["choices"][0]["text"] or "").strip())
            per_sample.append({"lang": f"{sample[0]}->{sample[1]}", "tokens": tokens(answer)})
        round_totals.append((total_tokens, total_seconds))

    best_tokens, best_seconds = max(round_totals, key=lambda pair: pair[0] / pair[1])
    digest = hashlib.sha256("\n".join(outputs).encode()).hexdigest()[:16]
    report = {
        "label": args.label,
        "port": args.port,
        "reps": args.reps,
        "concurrency": args.concurrency,
        "rounds": [round(tokens / seconds, 1) for tokens, seconds in round_totals],
        "tokens_per_second": round(best_tokens / best_seconds, 1),
        "tokens": best_tokens,
        "seconds": round(best_seconds, 3),
        "output_sha256": digest,
        "outputs": outputs,
    }

    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0

    if args.label:
        print(f"{args.label} (port {args.port}, concurrency {args.concurrency})")
    print(f"rounds: {report['rounds']} tok/s")
    print(f"best:   {report['tokens_per_second']} tok/s ({report['tokens']} tok / {report['seconds']} s)")
    print(f"output: {digest}")
    for index, sample in enumerate(per_sample[: len(SAMPLES)]):
        print(f"  {sample['lang']:6s} {sample['tokens']:3d} tok  {outputs[index][:48]!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
