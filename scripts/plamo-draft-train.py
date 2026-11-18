#!/usr/bin/env python3
"""Train the Token-Gated LoRA draft head for PLaMo 3 Translate (trap.jp/post/3079, ported).

The draft proposes several tokens in one forward; the frozen model verifies them in one forward, so
a cycle costs one 1-token forward plus one (block+1)-token forward and yields (accepted + 1) tokens.
Only the LoRA matrices train; the base weights are untouched, so the translation the base model
produces is unchanged by construction.

    # mechanics check on this Mac, no data download, 20 steps
    scripts/plamo-draft-train.py --smoke --device mps

    # a real run (Colab L4): ~10 M tokens
    scripts/plamo-draft-train.py --data hf:emiz6413/JESC --steps 1500 --batch-size 4 \
        --seq-len 384 --out /content/draft-head

Writes `draft_head.safetensors` (LoRA A/B only, ~30 MB), `draft_head.json` (rank, alpha, targets,
block size) and prints draft acceptance on held-out pairs.
"""

import argparse
import json
import math
import os
import random
import sys
import time

import torch
import torch.nn as nn
import torch.nn.functional as F

TARGETS = ("mixer.qkv_proj", "mixer.o_proj", "mlp.gate_up_proj", "mlp.down_proj")
POLITE = "<|plamo:key|>form<|plamo:val|>polite"
STOP = "<|plamo:tag|>"

# The gate the LoRA reads: 1 where a position drafts in parallel, 0 where the base model speaks.
GATE: dict[str, object] = {"value": None, "enabled": False}


def prompt_text(text: str, lang: str) -> str:
    polite = POLITE if lang == "ja" else ""
    return (
        "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>"
        f"{text}<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>{lang}{polite}<|plamo:msg|>"
    )


class GatedLoRA(nn.Module):
    """y = W x + g * (alpha/rank) * B A x, with g a per-token gate."""

    def __init__(self, base: nn.Linear, rank: int, alpha: float) -> None:
        super().__init__()
        self.base = base
        self.rank = rank
        self.scale = alpha / rank
        device = base.weight.device
        self.a = nn.Parameter(torch.zeros(rank, base.in_features, device=device))
        self.b = nn.Parameter(torch.zeros(base.out_features, rank, device=device))
        nn.init.kaiming_uniform_(self.a, a=math.sqrt(5))

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        y = self.base(x)
        if not GATE["enabled"]:
            return y
        # The LoRA matrices stay fp32 (as in the article); the activation is cast to meet them.
        hidden = x.to(self.a.dtype)
        lora = torch.matmul(torch.matmul(hidden, self.a.t()), self.b.t()) * self.scale
        gate = GATE["value"]
        return y if gate is None else y + (lora * gate.float()).to(y.dtype)


def attach(model: nn.Module, rank: int, alpha: float) -> list[str]:
    """Swap every target Linear for a gated LoRA wrapper; returns the wrapped module names."""
    wrapped: list[str] = []

    def walk(module: nn.Module, prefix: str) -> None:
        for name, child in list(module.named_children()):
            path = f"{prefix}.{name}" if prefix else name
            if isinstance(child, nn.Linear) and path.endswith(TARGETS):
                setattr(module, name, GatedLoRA(child, rank, alpha))
                wrapped.append(path)
            else:
                walk(child, path)

    walk(model, "")
    return wrapped


def load_model(path: str, device: str) -> tuple[nn.Module, object]:
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(path, trust_remote_code=True)
    try:
        model = AutoModelForCausalLM.from_pretrained(path, trust_remote_code=True, dtype=torch.bfloat16)
    except TypeError:  # transformers < 4.56 spells it torch_dtype
        model = AutoModelForCausalLM.from_pretrained(
            path, trust_remote_code=True, torch_dtype=torch.bfloat16
        )
    model.to(device)
    model.eval()
    for parameter in model.parameters():
        parameter.requires_grad_(False)
    return model, tokenizer


SYNTHETIC = [
    ("en", "ja", "The meeting has been rescheduled to next Monday due to a scheduling conflict."),
    ("ja", "en", "本日は晴天なり。開発環境のセットアップとテストを実行してください。"),
    ("en", "ja", "Apple Silicon unified memory lets the GPU read quantized weights directly."),
    ("ja", "en", "設定ファイルを変更したあとは、テストを実行してカバレッジを確認してください。"),
    ("en", "ja", "Run the tests before committing; the linter and the formatter run first."),
    ("ja", "en", "翻訳の品質を保ったまま、推論速度を改善する方法を検討しています。"),
    ("en", "ja", "The server listens on port 18555 and starts on demand."),
    ("ja", "en", "この関数は、与えられた文字列を指定された言語に翻訳します。"),
]


def load_pairs(spec: str, limit: int) -> list[tuple[str, str, str]]:
    """(source_lang, target_lang, text) triples; `hf:<dataset>` reads a parallel corpus."""
    if spec == "synthetic":
        return [(src, tgt, text) for src, tgt, text in SYNTHETIC]
    from datasets import load_dataset

    name = spec.split(":", 1)[1]
    dataset = load_dataset(name, split="train")
    pairs: list[tuple[str, str, str]] = []
    for row in dataset:
        # JESC and friends expose a Japanese and an English column under several names.
        japanese = row.get("ja") or row.get("japanese") or row.get("translation", {}).get("ja")
        english = row.get("en") or row.get("english") or row.get("translation", {}).get("en")
        if not japanese or not english:
            continue
        pairs.append(("ja", "en", japanese))
        pairs.append(("en", "ja", english))
        if len(pairs) >= limit:
            break
    return pairs


def build_batch(
    pairs: list[tuple[str, str, str]],
    tokenizer,
    seq_len: int,
    block: int,
    noise_fraction: float,
    rng: random.Random,
    device: str,
) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor] | None:
    """Clean ids (teacher), noised ids (draft), per-token gate, and the positions that carry loss."""
    tag = tokenizer.encode(STOP, add_special_tokens=False)[0]
    vocab = len(tokenizer)
    rows: list[tuple[list[int], list[int], list[int], list[int]]] = []
    for source_lang, target_lang, text in pairs:
        prompt_ids = tokenizer.encode(prompt_text(text, target_lang), add_special_tokens=False)
        target_ids = tokenizer.encode(text, add_special_tokens=False) + [tag]
        clean = prompt_ids + target_ids
        if len(clean) > seq_len or len(prompt_ids) >= len(clean):
            continue
        noisy = list(clean)
        gate = [0] * len(clean)
        positions: list[int] = []
        # Noise blocks start after the prompt, so a block always has clean context before it.
        start = len(prompt_ids)
        while start + block <= len(clean):
            if rng.random() < noise_fraction:
                for offset in range(block):
                    index = start + offset
                    noisy[index] = rng.randrange(vocab)
                    gate[index] = 1
                # The block's last position predicts a token past it, which the loss skips.
                positions.extend(range(start, min(start + block, len(clean) - 1)))
            start += block
        if not positions:
            continue
        rows.append((clean, noisy, gate, positions))

    if not rows:
        return None
    width = max(len(row[0]) for row in rows)
    clean_ids = torch.zeros((len(rows), width), dtype=torch.long)
    noisy_ids = torch.zeros((len(rows), width), dtype=torch.long)
    gates = torch.zeros((len(rows), width, 1), dtype=torch.bfloat16)
    loss_positions: list[list[int]] = []
    for index, (clean, noisy, gate, positions) in enumerate(rows):
        clean_ids[index, : len(clean)] = torch.tensor(clean)
        noisy_ids[index, : len(noisy)] = torch.tensor(noisy)
        gates[index, : len(gate), 0] = torch.tensor(gate, dtype=torch.bfloat16)
        loss_positions.append(positions)
    return (
        clean_ids.to(device),
        noisy_ids.to(device),
        gates.to(device),
        loss_positions,
    )


def l1_loss(teacher_logits: torch.Tensor, draft_logits: torch.Tensor) -> torch.Tensor:
    """Total-variation loss between the teacher and draft next-token distributions."""
    teacher = torch.softmax(teacher_logits.float(), dim=-1)
    draft = torch.log_softmax(draft_logits.float(), dim=-1)
    return (teacher * (torch.log(teacher.clamp_min(1e-12)) - draft)).sum(-1).mean()


def gather(logits: torch.Tensor, rows: list[list[int]], device: str) -> torch.Tensor:
    index = torch.tensor(
        [[row, position] for row, positions in enumerate(rows) for position in positions],
        device=device,
    )
    return logits[index[:, 0], index[:, 1]]


def train(args) -> None:
    torch.manual_seed(args.seed)
    random.seed(args.seed)
    model, tokenizer = load_model(args.model, args.device)
    wrapped = attach(model, args.rank, args.alpha)
    parameters = [p for p in model.parameters() if p.requires_grad]
    print(f"wrapped {len(wrapped)} projections, {sum(p.numel() for p in parameters):,} trainable params")

    pairs = load_pairs(args.data, args.pairs)
    if not pairs:
        raise SystemExit("no training pairs")
    rng = random.Random(args.seed)
    optimizer = torch.optim.AdamW(parameters, lr=args.lr, weight_decay=0.0)
    model.train()
    started = time.perf_counter()
    tokens = 0
    running = 0.0

    for step in range(1, args.steps + 1):
        batch = build_batch(
            rng.sample(pairs, min(len(pairs), args.batch_size * 2)),
            tokenizer,
            args.seq_len,
            args.block,
            args.noise_fraction,
            rng,
            args.device,
        )
        if batch is None:
            continue
        clean_ids, noisy_ids, gates, positions = batch

        # Teacher: the frozen model's distribution on the clean tokens.
        GATE["enabled"] = False
        with torch.no_grad():
            teacher = model(input_ids=clean_ids).logits

        # Draft: the same tokens where clean, random tokens inside the noise blocks, gate open there.
        GATE["enabled"] = True
        GATE["value"] = gates
        draft = model(input_ids=noisy_ids).logits
        GATE["value"] = None
        GATE["enabled"] = False

        loss = l1_loss(gather(teacher, positions, args.device), gather(draft, positions, args.device))
        (loss / args.grad_accum).backward()
        running += loss.item()
        tokens += int(sum(len(row) for row in positions))

        if step % args.grad_accum == 0:
            torch.nn.utils.clip_grad_norm_(parameters, 1.0)
            optimizer.step()
            optimizer.zero_grad(set_to_none=True)
            if step % args.log_every == 0:
                elapsed = time.perf_counter() - started
                print(
                    f"step {step:5d}  loss {running / args.log_every:.4f}  "
                    f"{tokens / elapsed:7.0f} loss-positions/s  {elapsed:6.0f}s",
                    flush=True,
                )
                running = 0.0
        if step % args.save_every == 0:
            save(model, tokenizer, args, wrapped, step)

    save(model, tokenizer, args, wrapped, args.steps)
    measure_acceptance(model, tokenizer, args, pairs)


def save(model, tokenizer, args, wrapped: list[str], step: int) -> None:
    os.makedirs(args.out, exist_ok=True)
    state = {
        name: {"a": module.a.detach().cpu(), "b": module.b.detach().cpu()}
        for name, module in model.named_modules()
        if isinstance(module, GatedLoRA)
    }
    torch.save(state, os.path.join(args.out, "draft_head.pt"))
    with open(os.path.join(args.out, "draft_head.json"), "w", encoding="utf-8") as handle:
        json.dump(
            {
                "rank": args.rank,
                "alpha": args.alpha,
                "block": args.block,
                "targets": list(TARGETS),
                "wrapped": wrapped,
                "step": step,
                "model": args.model,
            },
            handle,
            indent=2,
        )
    print(f"saved {len(state)} LoRA matrices to {args.out} (step {step})", flush=True)


def propose(model, tokenizer, prompt_ids: torch.Tensor, block: int, device: str) -> list[int]:
    """One draft forward proposes `block` tokens from the end of the prompt."""
    vocab = len(tokenizer)
    noise = torch.randint(0, vocab, (1, block - 1), device=device)
    ids = torch.cat([prompt_ids, noise], dim=1)
    gate = torch.zeros((1, ids.shape[1], 1), dtype=torch.bfloat16, device=device)
    gate[0, prompt_ids.shape[1] - 1 :, 0] = 1  # the last clean position and every noise position
    GATE["enabled"] = True
    GATE["value"] = gate
    with torch.no_grad():
        logits = model(input_ids=ids).logits
    GATE["enabled"] = False
    GATE["value"] = None
    start = prompt_ids.shape[1] - 1
    return [int(torch.argmax(logits[0, start + offset])) for offset in range(block)]


def verify(model, tokenizer, prompt_ids: torch.Tensor, candidates: list[int], device: str) -> int:
    """Greedy verification: how many proposed tokens the frozen model would itself pick."""
    ids = torch.cat([prompt_ids, torch.tensor([candidates], device=device)], dim=1)
    GATE["enabled"] = False
    with torch.no_grad():
        logits = model(input_ids=ids).logits
    start = prompt_ids.shape[1] - 1
    accepted = 0
    for offset, candidate in enumerate(candidates):
        if int(torch.argmax(logits[0, start + offset])) != candidate:
            break
        accepted += 1
    return accepted


def measure_acceptance(model, tokenizer, args, pairs) -> None:
    rng = random.Random(args.seed + 1)
    samples = rng.sample(pairs, min(len(pairs), args.accept_samples))
    accepted = 0
    cycles = 0
    for _source_lang, target_lang, text in samples:
        prompt_ids = torch.tensor(
            [tokenizer.encode(prompt_text(text, target_lang), add_special_tokens=False)],
            device=args.device,
        )
        candidates = propose(model, tokenizer, prompt_ids, args.block, args.device)
        accepted += verify(model, tokenizer, prompt_ids, candidates, args.device)
        cycles += 1
    if cycles:
        per_cycle = accepted / cycles
        print(
            f"draft acceptance: {per_cycle:.2f}/{args.block} tokens per cycle "
            f"({cycles} samples) -> {(per_cycle + 1):.2f} tokens per draft+verify pair",
            flush=True,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="pfnet/plamo-3-translate-0.8b")
    parser.add_argument("--data", default="synthetic", help="synthetic, or hf:<dataset>")
    parser.add_argument("--pairs", type=int, default=20000)
    parser.add_argument("--out", default="draft-head")
    parser.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    parser.add_argument("--steps", type=int, default=1500)
    parser.add_argument("--batch-size", type=int, default=4)
    parser.add_argument("--grad-accum", type=int, default=2)
    parser.add_argument("--seq-len", type=int, default=384)
    parser.add_argument("--block", type=int, default=8)
    parser.add_argument("--noise-fraction", type=float, default=0.5)
    parser.add_argument("--rank", type=int, default=16)
    parser.add_argument("--alpha", type=float, default=16.0)
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--log-every", type=int, default=10)
    parser.add_argument("--save-every", type=int, default=200)
    parser.add_argument("--accept-samples", type=int, default=24)
    parser.add_argument("--smoke", action="store_true", help="20 steps on the built-in pairs")
    args = parser.parse_args()
    if args.smoke:
        args.steps = 20
        args.save_every = 20
        args.accept_samples = 8
        args.seq_len = 192
    train(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
