#!/usr/bin/env python3
"""Prompt-lookup speculative decoding vs plain greedy decode, on the real 4-bit checkpoint.

    scripts/bench-plamo-lookup.py          # 4 input classes, tok/s and token-id equality

The draft is the k tokens that followed the longest earlier occurrence of the context's suffix
(the prompt, then everything generated so far). One forward pass over [accepted token] + draft
verifies it: the longest leading run where the model's argmax equals the drafted token is kept,
and the verify pass becomes the new cache state, so a fully accepted draft costs one pass.

Measured on this Mac (M5 Pro, mlx-lm 0.32.4, k=8, 64 tokens):

    repeated block  plain 338.7  lookup 594.8 tok/s  x1.76  identical
    ui strings      plain 348.7  lookup 267.2 tok/s  x0.77  identical
    prose           plain 338.9  lookup 274.5 tok/s  x0.81  identical
    ja prose (en)   plain 342.2  lookup 256.3 tok/s  x0.75  token 39 differs (180 vs 181)

Not shipped in scripts/plamo-mlx-server.py, for two reasons the bench reports itself:

1. It is not output-identical. A k-token pass does not reproduce the logits of k single-token
   passes on this model: mlx-lm writes a multi-token pass through RotatingKVCache's concatenating
   path and a single-token pass in place, so the KV buffers differ in size, the attention kernel
   takes a different path, and the fp16 accumulation drifts by ~0.4 logits on a scale of 32
   (~1.5%). Greedy argmax usually survives that, but not always — here the ja prose case flips one
   token. Every rewind mechanism tried (trim_prompt_cache, deepcopy, copying the cache buffers)
   produced token mismatches on at least one input class, so no variant of this is byte-identical.
2. Even ignoring identity, the mods' main workload does not win: drafting only pays when the
   draft is mostly accepted, and translations of ordinary text are not repetitive enough for an
   n-gram match to hit often. Only repeated blocks gain (x1.76); UI strings and prose get slower
   than plain decode, because a missed draft costs a whole extra pass.
"""

import argparse
import copy
import time
from pathlib import Path

import mlx.core as mx
from mlx_lm.models.cache import make_prompt_cache
from mlx_lm.utils import load_model
from transformers import AutoTokenizer

MODEL = "~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
POLITE = "<|plamo:key|>form<|plamo:val|>polite"

REPEATED = "The build finished at 10:42.\n" * 6
UI_STRINGS = (
    "Save changes | Cancel | Delete | Rename | Duplicate | Save changes | Cancel | Delete\n"
    "Settings > Appearance > Theme: Light | Dark | System | Settings > Appearance > Theme"
)
PROSE = (
    "The meeting moved to Thursday because the client asked for one more day to review the "
    "figures, and nobody objected."
)
JA_PROSE = "会議は、クライアントが数値の確認にもう1日必要だと言ったため、木曜日に変更されました。"
# Input classes: a repeated line block, repetitive UI strings, plain English prose and plain
# Japanese prose. The language is the output language, as the mods request it.
CASES = [
    ("repeated block", REPEATED, "ja"),
    ("ui strings", UI_STRINGS, "ja"),
    ("prose", PROSE, "ja"),
    ("ja prose", JA_PROSE, "en"),
]


def prompt(text: str, lang: str) -> str:
    """The PFML3 prompt the mods send (tools/claude-plamo-translate/hooks/translate.ts)."""
    return (
        "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>"
        f"{text}<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>{lang}"
        f"{POLITE if lang == 'ja' else ''}<|plamo:msg|>"
    )


def lookup(context: list[int], k: int, longest: int = 32) -> list[int]:
    """The k tokens after the longest earlier occurrence of the context's suffix, or []."""
    for size in range(min(len(context) - 1, longest), 0, -1):
        suffix = context[-size:]
        for start in range(len(context) - size - 1, -1, -1):
            if context[start : start + size] == suffix:
                return context[start + size : start + size + k]
    return []


def clone(cache):
    """A cache with independent buffers: an mx.array is mutable through slice assignment."""
    copies = []
    for entry in cache:
        new = copy.copy(entry)
        new.state = tuple(
            mx.array(part) if isinstance(part, mx.array) else part for part in entry.state
        )
        copies.append(new)
    return copies


def plain(model, ids: list[int], limit: int) -> list[int]:
    cache = make_prompt_cache(model)
    logits = model(mx.array([ids]), cache=cache)[:, -1, :]
    out = []
    for _ in range(limit):
        token = int(mx.argmax(logits[0]))
        out.append(token)
        logits = model(mx.array([[token]]), cache=cache)[:, -1, :]
    return out


def speculative(model, ids: list[int], limit: int, k: int = 8) -> tuple[list[int], int, int]:
    """Greedy decode with prompt-lookup drafting. Returns (tokens, verify passes, accepted)."""
    cache = make_prompt_cache(model)
    logits = model(mx.array([ids]), cache=cache)[:, -1, :]
    context = list(ids)
    out: list[int] = []
    passes = accepted_total = 0
    skip = 0
    while len(out) < limit:
        token = int(mx.argmax(logits[0]))
        out.append(token)
        context.append(token)
        if len(out) >= limit:
            break
        # A missed draft costs a wasted pass, so skip drafting for two steps after one.
        skip = max(0, skip - 1)
        draft = lookup(context, k) if skip == 0 else []
        if not draft:
            logits = model(mx.array([[token]]), cache=cache)[:, -1, :]
            continue
        verify_cache = clone(cache)
        verify = model(mx.array([[token] + draft]), cache=verify_cache)
        passes += 1
        matched = 0
        while matched < len(draft) and int(mx.argmax(verify[0, matched])) == draft[matched]:
            matched += 1
        accepted_total += matched
        if matched == len(draft):
            # The verify pass already is the next state, so nothing is fed twice.
            cache, logits = verify_cache, verify[:, -1:, :]
        else:
            skip = 2
            logits = model(mx.array([[token] + draft[:matched]]), cache=cache)[:, -1:, :]
        out.extend(draft[:matched])
        context.extend(draft[:matched])
    return out[:limit], passes, accepted_total


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default=MODEL)
    parser.add_argument("--max-tokens", type=int, default=64)
    parser.add_argument("--k", type=int, default=8, help="draft length (default 8)")
    parser.add_argument("--json", action="store_true", help="print JSON instead of a table")
    args = parser.parse_args()

    # mlx_lm.load does not forward trust_remote_code to the tokenizer, and this checkpoint ships
    # its own tokenization code.
    model, _ = load_model(Path(args.model).expanduser(), trust_remote_code=True)
    tokenizer = AutoTokenizer.from_pretrained(
        str(Path(args.model).expanduser()), trust_remote_code=True
    )

    reports = []
    for name, text, lang in CASES:
        ids = tokenizer.encode(prompt(text, lang), add_special_tokens=False)
        plain(model, ids, 8)  # compile the Metal shaders before timing
        started = time.perf_counter()
        expected = plain(model, ids, args.max_tokens)
        plain_seconds = time.perf_counter() - started
        started = time.perf_counter()
        got, passes, accepted = speculative(model, ids, args.max_tokens, args.k)
        spec_seconds = time.perf_counter() - started
        report = {
            "case": name,
            "plain_tok_s": round(len(expected) / plain_seconds, 1),
            "lookup_tok_s": round(len(got) / spec_seconds, 1),
            "speedup": round(plain_seconds / spec_seconds, 2),
            "identical": expected == got,
            "verify_passes": passes,
            "accepted": accepted,
            "text": tokenizer.decode(got)[:60],
        }
        reports.append(report)
        if not args.json:
            print(
                f"{name:15s} plain {report['plain_tok_s']:6.1f}  "
                f"lookup {report['lookup_tok_s']:6.1f} tok/s  "
                f"x{report['speedup']:4.2f}  identical={report['identical']}  "
                f"passes {passes} accepted {accepted}",
                flush=True,
            )
            if not report["identical"]:
                for index, (a, b) in enumerate(zip(expected, got)):
                    if a != b:
                        print(f"  first difference at {index}: {a} != {b}", flush=True)
                        break
                print(f"  lengths {len(expected)} vs {len(got)}", flush=True)

    if args.json:
        import json

        print(json.dumps(reports, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
