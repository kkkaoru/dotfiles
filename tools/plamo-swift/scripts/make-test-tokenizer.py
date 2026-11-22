#!/usr/bin/env python3
"""Build the tiny tokenizer fixture the Swift tests load.

The Swift tokenizer replays the trie that dump-tokenizer.py dumps from the reference tokenizer, so
the fixture is produced by that same reference over a vocabulary small enough to commit: 256 byte
tokens plus a handful of words. Regenerate it only when the reference tokenizer changes.

  make-test-tokenizer.py [--model <dir>] [--out <fixtures dir>]
"""
import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_MODEL = Path.home() / ".local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
TOKENS = [
    ("<|plamo:unk|>", 0.0, "UNKNOWN"),
    ("<|plamo:bos|>", 0.0, "CONTROL"),
    ("<|plamo:eos|>", 0.0, "CONTROL"),
    ("<|plamo:pad|>", 0.0, "CONTROL"),
    ("hello", -1.5, None),
    (" world", -2.0, None),
    ("a", -3.0, None),
    ("ab", -2.5, None),
    ("bc", -3.5, None),
    ("▁", -4.0, None),
    ("日本語", -2.0, None),
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, default=DEFAULT_MODEL)
    parser.add_argument("--out", type=Path, default=HERE.parent / "Tests/PlamoCoreTests/Fixtures")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        (work / "tokenization_plamo.py").symlink_to(args.model / "tokenization_plamo.py")
        rows = [[token, score] if kind is None else [token, score, kind] for token, score, kind in TOKENS]
        rows += [[f"<0x{byte:02X}>", 0.0, "BYTE"] for byte in range(256)]
        with (work / "tokenizer.jsonl").open("w", encoding="utf-8") as handle:
            for row in rows:
                handle.write(json.dumps(row, ensure_ascii=False) + "\n")
        subprocess.run(
            [
                sys.executable,
                str(HERE / "dump-tokenizer.py"),
                str(work),
                str(args.out / "mini-tokenizer.bin"),
            ],
            check=True,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
