#!/usr/bin/env python3
"""Top-5 logits of the last prompt position from mlx_lm, to compare with the Swift port.

  reference-logits.py <model dir> <prompt>
"""
import json
import sys

import mlx.core as mx
from mlx_lm import load

path, prompt = sys.argv[1], sys.argv[2]
model, tokenizer = load(path, tokenizer_config={"trust_remote_code": True}, trust_remote_code=True)
ids = list(tokenizer.encode(prompt))
logits = model(mx.array([ids]))[0, -1]
top = mx.argsort(-logits)[:5]
print(
    json.dumps(
        {
            "ids": ids,
            "top": [[int(i), float(logits[int(i)])] for i in top],
        },
        ensure_ascii=False,
    )
)
