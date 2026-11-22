#!/usr/bin/env python3
"""Compare greedy output of the Swift server with mlx_lm on a fixed prompt set.

  check-parity.py <model dir> <swift binary> <tokenizer blob>
"""
import json
import subprocess
import sys

from mlx_lm import generate, load

PROMPTS = [
    ("ja->en", "こんにちは、世界。"),
    ("en->ja", "Hello, world."),
    ("long", "The quick brown fox jumps over the lazy dog and keeps running into the woods."),
    ("code", "def add(a, b):\n    return a + b"),
    ("mixed", "MLX で推論を速くしたい。Memory は 1 GB 未満に抑えたい。"),
]
MAX_TOKENS = 24


def prompt_text(body: str) -> str:
    return (
        "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>"
        + body
        + "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>"
    )


def main() -> int:
    path, binary, blob = sys.argv[1], sys.argv[2], sys.argv[3]
    model, tokenizer = load(path, tokenizer_config={"trust_remote_code": True}, trust_remote_code=True)
    failures = 0
    for name, body in PROMPTS:
        text = prompt_text(body)
        reference = generate(model, tokenizer, prompt=text, max_tokens=MAX_TOKENS, verbose=False)
        out = subprocess.run(
            [binary, "generate", "--model", path, "--tokenizer", blob,
             "--max-tokens", str(MAX_TOKENS), text],
            capture_output=True, text=True, check=True,
        ).stdout.splitlines()
        swift = "\n".join(out[2:])
        ok = swift == reference
        failures += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {json.dumps(swift[:60], ensure_ascii=False)}")
        if not ok:
            print(f"     python: {json.dumps(reference[:60], ensure_ascii=False)}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
