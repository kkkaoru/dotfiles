"""H1: could prompt-lookup decoding propose tokens for this workload?

Prompt-lookup decoding (Saxena 2023, arXiv:2304.04431) drafts the next k tokens by matching the last
n tokens against the prompt, and reports 2.4x on summarization and context-QA. It works in token
space, so this measures the same thing in token space: for real greedy translations of the mods'
workload, how many output n-grams already occur in the prompt, and how many repeat in the output
itself (the self-drafting fallback).
"""

import json
import os
import urllib.request

from transformers import AutoTokenizer

MODEL_DIR = f"{os.path.expanduser('~')}/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
PORT = 18555
TEMPLATE = (
    "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>{text}"
    "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>ja<|plamo:key|>form<|plamo:val|>polite"
    "<|plamo:msg|>"
)
PROMPTS = [
    "Run the tests before you commit.",
    "The server is still starting up, please wait a moment.",
    "Delete the old log files after the migration finishes.",
    "This function returns null when the user is not found.",
    "Open the settings panel and disable automatic updates.",
    "The build failed because the cache directory was missing.",
    "Can you check whether the tunnel is still connected?",
    "We should split this change into two smaller commits.",
    "The download resumed where it had stopped.",
    "Make sure the encrypted secret is never committed.",
    "I restarted the machine and the service came back by itself.",
    "That number is the physical footprint, not the resident size.",
    "Please summarize the discussion in three bullet points.",
    "The idle timer stops the model after ten minutes.",
    "Copy the token into the environment file on the other machine.",
    "The panic happened again with the same assertion id.",
    "This table shows the decode time per token.",
    "Leave the queue alone until the batch finishes.",
    "The weights are memory mapped, so no copy is made.",
    "Try the same request again with the cache disabled.",
]
N = 3


def translate(text: str) -> str:
    body = json.dumps(
        {
            "prompt": TEMPLATE.format(text=text),
            "max_tokens": 64,
            "temperature": 0,
            "stop": ["<|plamo:tag|>", "<|plamo:bos|>"],
            "cache": False,
        }
    ).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/v1/completions",
        data=body,
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=120) as reply:
        return json.loads(reply.read())["choices"][0]["text"]


def covered_by(output: list[int], grams: set[tuple[int, ...]]) -> int:
    covered: set[int] = set()
    for index in range(len(output) - N + 1):
        if tuple(output[index : index + N]) in grams:
            covered.update(range(index, index + N))
    return len(covered)


def from_prompt(output: list[int], prompt: list[int]) -> int:
    """Output tokens covered by an n-gram that also occurs in the prompt."""
    grams = {tuple(prompt[i : i + N]) for i in range(len(prompt) - N + 1)}
    return covered_by(output, grams)


def from_self(output: list[int]) -> int:
    """Output tokens covered by an n-gram that occurs at least twice, i.e. self-draftable."""
    counts: dict[tuple[int, ...], int] = {}
    for index in range(len(output) - N + 1):
        gram = tuple(output[index : index + N])
        counts[gram] = counts.get(gram, 0) + 1
    return covered_by(output, {gram for gram, count in counts.items() if count > 1})


def main() -> None:
    tokenizer = AutoTokenizer.from_pretrained(MODEL_DIR, trust_remote_code=True)
    rows = []
    for text in PROMPTS:
        prompt = TEMPLATE.format(text=text)
        out = translate(text)
        prompt_ids = tokenizer(prompt)["input_ids"]
        output_ids = tokenizer(out)["input_ids"]
        rows.append(
            {
                "input": text,
                "output": out.strip(),
                "tokens": len(output_ids),
                "from_prompt": from_prompt(output_ids, prompt_ids),
                "from_self": from_self(output_ids),
            }
        )
    total = sum(row["tokens"] for row in rows)
    prompt_hits = sum(row["from_prompt"] for row in rows)
    self_hits = sum(row["from_self"] for row in rows)
    print(
        json.dumps(
            {
                "prompts": len(rows),
                "output_tokens": total,
                "mean_tokens": round(total / len(rows), 1),
                "ngram": N,
                "draftable_from_prompt": prompt_hits,
                "draftable_from_self": self_hits,
                "share_from_prompt": round(prompt_hits / total, 3),
                "share_from_self": round(self_hits / total, 3),
                "rows": rows[:4],
            },
            indent=2,
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
