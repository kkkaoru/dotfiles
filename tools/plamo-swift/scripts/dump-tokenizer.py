#!/usr/bin/env python3
"""Dump PLaMo 3's Unigram trie into a blob the Swift server can load.

The tokenizer's own build step is the expensive, subtle part (suffix trie, scores, byte tokens), so
it runs once here, in Python, and Swift only replays the Viterbi walk over the same arrays.

  dump-tokenizer.py <model dir> <output blob>
"""
import importlib.util
import json
import struct
import sys
from pathlib import Path


def load_module(model_dir: Path):
    spec = importlib.util.spec_from_file_location(
        "tokenization_plamo", model_dir / "tokenization_plamo.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules["tokenization_plamo"] = module
    spec.loader.exec_module(module)
    return module


def main() -> int:
    model_dir = Path(sys.argv[1])
    out = Path(sys.argv[2])
    plamo = load_module(model_dir)
    tokenizer = plamo.Plamo3Tokenizer(
        vocab_file=str(model_dir / "tokenizer.jsonl"),
        break_around_consecutive_spaces_threshold=2,
        break_around_repeated_chars_threshold=4,
    )
    trie = tokenizer.aho_corasick
    table = trie._table
    pairs = sorted((int(k), int(v)) for k, v in trie._to_suffix_id.items())
    tokens = trie._tokens

    with out.open("wb") as f:
        f.write(b"PLT1")
        f.write(struct.pack("<i", table.shape[0]))
        f.write(table.astype("<i4").tobytes())
        f.write(struct.pack("<i", len(pairs)))
        for key, value in pairs:
            f.write(struct.pack("<qi", key, value))
        f.write(trie._bytes.astype("<i4").tobytes())
        f.write(struct.pack("<i", len(tokens)))
        for token in tokens:
            raw = token.encode()
            f.write(struct.pack("<i", len(raw)))
            f.write(raw)

    meta = {
        "boundary_char": plamo.BOUNDARY_CHAR,
        "boundary_token_id": plamo.BOUNDARY_TOKEN_ID,
        "unknown_score": plamo.UNKNOWN_SCORE,
        "invalid_score": plamo.INVALID_SCORE,
        "special_pattern": r"(<\|plamo:[^|\s]{0,64}\|>)",
        "break_pattern": r"((.)\2{3,}| {2,})",
        "tokens": len(tokens),
        "table_rows": int(table.shape[0]),
    }
    out.with_suffix(".json").write_text(json.dumps(meta, indent=2) + "\n")
    print(f"{out}: {out.stat().st_size / 1e6:.1f} MB, {meta['table_rows']} rows, {len(tokens)} tokens")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
