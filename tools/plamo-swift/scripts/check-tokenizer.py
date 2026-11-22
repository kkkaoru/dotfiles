#!/usr/bin/env python3
"""Compare the Swift tokenizer with the Python one on a fixed prompt set.

  check-tokenizer.py <model dir> <blob> <plamo-swift binary>
"""
import importlib.util
import subprocess
import sys
from pathlib import Path

TEXTS = [
    "Translate the following text into English.",
    "こんにちは、世界。今日はいい天気ですね。",
    "Mixed 日本語 and English 12345 with punctuation!?",
    "<|plamo:msg|>already a special token<|plamo:tag|>",
    "a    b",  # consecutive spaces hit the boundary matcher
    "looooooooooong",  # repeated characters hit the boundary matcher
    "def f(x):\n    return x * 2  # comment\n",
    "  leading and trailing  ",
    "emoji 😀🚀 and combining e\u0301",
    "",
    "\n\n",
    "é" * 3,
    "A" * 4 + "B",
    "終わり。",
]


def load(model_dir: Path):
    spec = importlib.util.spec_from_file_location(
        "tokenization_plamo", model_dir / "tokenization_plamo.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules["tokenization_plamo"] = module
    spec.loader.exec_module(module)
    return module.Plamo3Tokenizer(
        vocab_file=str(model_dir / "tokenizer.jsonl"),
        break_around_consecutive_spaces_threshold=2,
        break_around_repeated_chars_threshold=4,
    )


def main() -> int:
    model_dir, blob, binary = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
    tokenizer = load(model_dir)
    failures = 0
    for text in TEXTS:
        expected = [int(i) for i in tokenizer.encode(text)]
        out = subprocess.run(
            [binary, "tokenize", "--model", str(model_dir), "--tokenizer", blob, text],
            capture_output=True,
            text=True,
            check=True,
        )
        actual = [int(i) for i in out.stdout.split()] if out.stdout.strip() else []
        status = "ok" if actual == expected else "MISMATCH"
        if actual != expected:
            failures += 1
            print(f"{status}: {text!r}\n  python: {expected}\n  swift:  {actual}")
        else:
            print(f"{status}: {text!r} -> {len(expected)} tokens")
    print(f"{len(TEXTS) - failures}/{len(TEXTS)} texts match")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
