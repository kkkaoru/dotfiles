#!/usr/bin/env python3
"""Greedy reference output from mlx_lm, to compare the Swift port against.

  reference.py <model dir> <prompt> <max tokens>
"""
import json
import sys

from mlx_lm import load, stream_generate
from mlx_lm.sample_utils import make_sampler

path, prompt, max_tokens = sys.argv[1], sys.argv[2], int(sys.argv[3])
model, tokenizer = load(
    path, tokenizer_config={"trust_remote_code": True}, trust_remote_code=True
)

ids, text = [], ""
for response in stream_generate(
    model, tokenizer, prompt, max_tokens=max_tokens, sampler=make_sampler(temp=0.0)
):
    ids.append(int(response.token))
    text += response.text
print(json.dumps({"prompt_ids": [int(i) for i in tokenizer.encode(prompt)], "ids": ids, "text": text}, ensure_ascii=False))
