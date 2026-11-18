#!/usr/bin/env python3
"""Distil a tiny draft model from PLaMo 3 Translate, for speculative decoding on Apple Silicon.

Why not the article's Token-Gated LoRA (scripts/plamo-draft-train.py): on this Mac a draft that
shares the 0.8B weights costs as much per proposal as a verify step, so a block-8 cycle needs ~42%
acceptance just to break even, and the article's trained draft reached 30%. A separate 2-4 layer
student reads ~1/8 of the bytes, so break-even drops to ~1.5/8 (19%) and 2.4/8 is ~1.4x.

The student is the teacher's first `--student-layers` layers plus its embedding, norm and head, so
it starts already knowing the language pair and only has to learn to draft a block in one forward.
The teacher is frozen: it runs on the clean sequence and the student, on a copy with the block's
tokens replaced by noise, matches its distribution at those positions (total-variation loss). The
translation the teacher produces is untouched by construction.

    # mechanics check on this Mac, 20 steps, no data download
    scripts/plamo-draft-distill.py --smoke --device mps

    # a real run (Colab L4): JESC, 2 layers, a few hours of pairs
    scripts/plamo-draft-distill.py --data hf:emiz6413/JESC --steps 8000 --batch-size 8 \
        --seq-len 384 --student-layers 2 --out /content/draft-distil

Writes `student.safetensors` (2-4 layers plus the shared head), `student.json` (layers, block size,
tokenizer, step) and prints acceptance for a fresh draft+verify cycle.

Measured on a Colab L4 (2026-11-19), 2 layers, JESC, batch 8, seq 384:

| run | steps | lr | TV loss | acceptance |
|---|---|---|---|---|
| 1 | 2,000 | 1e-4 | 1.83 -> 1.73 (then back up: bf16 weights cannot take a 1e-4 step) | 0.00/8 |
| 2 | 2,400 | 5e-4 | 1.88 -> 1.80, flat | 0.00/8 |

Break-even here is 1.5/8 (the verify pass costs 6.0 ms for 9 tokens against 2.66 ms per token, plus
~0.6 ms to draft), so the student has to be right about one token in five. It is right about none:
the teacher's own layers expect clean input, and learning to guess a whole block from noise is the
article's 0.75B-token problem. This machine sustains ~220 loss-positions/s on an L4, i.e. ~0.8M/h,
so 0.75B would take weeks (~1,900 CU) — not reachable in a 2-4 CU run. The draft-head path is
therefore closed by measurement, not by preference; the shipped speed-ups are the completion cache
and the continuous batching already in mlx_lm.server.
"""

import argparse
import copy
import json
import os
import random
import sys
import time

import torch
import torch.nn.functional as F

STOP = "<|plamo:tag|>"
POLITE = "<|plamo:key|>form<|plamo:val|>polite"

SYNTHETIC = [
    ("en", "ja", "The meeting has been rescheduled to next Monday.", "会議は来週の月曜日に変更されました。"),
    ("ja", "en", "本日は晴天なり。", "It is sunny today."),
    ("en", "ja", "Run the tests before committing.", "コミットする前にテストを実行してください。"),
    ("ja", "en", "サーバーは必要に応じて起動します。", "The server starts on demand."),
]


def prompt_text(text: str, lang: str) -> str:
    polite = POLITE if lang == "ja" else ""
    return (
        "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>"
        f"{text}<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>{lang}{polite}<|plamo:msg|>"
    )


def load_model(path: str, device: str):
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(path, trust_remote_code=True)
    try:
        model = AutoModelForCausalLM.from_pretrained(path, trust_remote_code=True, dtype=torch.bfloat16)
    except TypeError:  # transformers < 4.56 spells it torch_dtype
        model = AutoModelForCausalLM.from_pretrained(path, trust_remote_code=True, torch_dtype=torch.bfloat16)
    model.to(device)
    model.eval()
    for parameter in model.parameters():
        parameter.requires_grad_(False)
    return model, tokenizer


def build_student(teacher, layers: int, device: str):
    """The teacher's first `layers` layers with its embedding, norm and head."""
    from transformers import AutoModelForCausalLM

    config = copy.deepcopy(teacher.config)
    config.num_hidden_layers = layers
    student = AutoModelForCausalLM.from_config(config, trust_remote_code=True)
    source = teacher.state_dict()
    copied = 0
    with torch.no_grad():
        for name, tensor in student.state_dict().items():
            origin = source.get(name)
            if origin is not None and origin.shape == tensor.shape:
                tensor.copy_(origin)
                copied += 1
    print(f"student: {layers} layers, {copied} tensors taken from the teacher", flush=True)
    student.to(device)
    student.train()
    # The embedding and the head stay the teacher's: they are 330M of the 400M parameters, the
    # draft's vocabulary distribution should not drift from the one verifying it, and only the
    # layers decide how well a block can be guessed.
    for name, parameter in student.named_parameters():
        if name.startswith(("lm_head.", "model.embed_tokens.")):
            parameter.requires_grad_(False)
    # bf16 weights cannot absorb a 1e-4 update (its epsilon is coarser than the step), so the
    # student trains in fp32 while the frozen teacher stays bf16.
    return student.float()


def load_pairs(spec: str, limit: int) -> list[tuple[str, str, str, str]]:
    """(source_lang, target_lang, source_text, target_text); `hf:<dataset>` reads a parallel corpus."""
    if spec == "synthetic":
        return list(SYNTHETIC)
    from datasets import load_dataset

    # The split slice keeps a 2.8M-row corpus from downloading in full.
    dataset = load_dataset(spec.split(":", 1)[1], split=f"train[:{limit}]")
    pairs: list[tuple[str, str, str, str]] = []
    for row in dataset:
        # JESC (and other subtitle corpora) ship one column of "en\tja" lines; a parallel corpus
        # ships a column per language, sometimes nested under "translation".
        columns = row.get("translation") if isinstance(row.get("translation"), dict) else row
        english, japanese = columns.get("en"), columns.get("ja")
        if english is None and japanese is None and isinstance(row.get("text"), str):
            english, _, japanese = row["text"].partition("\t")
        english = english or columns.get("english")
        japanese = japanese or columns.get("japanese")
        if not japanese or not english:
            continue
        pairs.append(("ja", "en", japanese, english))
        pairs.append(("en", "ja", english, japanese))
        if len(pairs) >= limit:
            break
    return pairs


def build_batch(pairs, tokenizer, seq_len: int, block: int, noise_fraction: float, rng, device: str):
    """Clean ids for the teacher, the same ids with whole blocks noised for the student, and the
    positions whose teacher distribution the student has to match."""
    tag = tokenizer.encode(STOP, add_special_tokens=False)[0]
    vocab = len(tokenizer)
    rows = []
    for source_lang, target_lang, source_text, target_text in pairs:
        prompt_ids = tokenizer.encode(prompt_text(source_text, target_lang), add_special_tokens=False)
        target_ids = tokenizer.encode(target_text, add_special_tokens=False) + [tag]
        clean = prompt_ids + target_ids
        if len(clean) > seq_len or len(prompt_ids) >= len(clean) - 1:
            continue
        noisy = list(clean)
        positions: list[int] = []
        # Blocks start after the prompt, so each one has clean context before it.
        start = len(prompt_ids)
        while start + block <= len(clean):
            if rng.random() < noise_fraction:
                for offset in range(block):
                    noisy[start + offset] = rng.randrange(vocab)
                # The block's last position predicts a token past it, which the loss skips.
                positions.extend(range(start, min(start + block, len(clean) - 1)))
            start += block
        if not positions:
            continue
        rows.append((clean, noisy, positions))

    if not rows:
        return None
    width = max(len(row[0]) for row in rows)
    clean_ids = torch.zeros((len(rows), width), dtype=torch.long)
    noisy_ids = torch.zeros((len(rows), width), dtype=torch.long)
    mask = torch.zeros((len(rows), width), dtype=torch.bool)
    for index, (clean, noisy, positions) in enumerate(rows):
        clean_ids[index, : len(clean)] = torch.tensor(clean)
        noisy_ids[index, : len(noisy)] = torch.tensor(noisy)
        mask[index, positions] = True
    return clean_ids.to(device), noisy_ids.to(device), mask.to(device)


def tv_loss(teacher_logits: torch.Tensor, student_logits: torch.Tensor) -> torch.Tensor:
    """Total variation between the two next-token distributions (the article's L1/TV objective)."""
    teacher = torch.softmax(teacher_logits.float(), dim=-1)
    student = torch.softmax(student_logits.float(), dim=-1)
    return (teacher - student).abs().sum(dim=-1).mean()


def propose(student, tokenizer, prompt_ids: torch.Tensor, block: int, device: str) -> list[int]:
    """The student drafts `block` tokens from the end of the prompt, in one forward."""
    vocab = len(tokenizer)
    noise = torch.randint(0, vocab, (1, block - 1), device=device)
    ids = torch.cat([prompt_ids, noise], dim=1)
    with torch.no_grad():
        logits = student(input_ids=ids).logits
    start = prompt_ids.shape[1] - 1
    return [int(torch.argmax(logits[0, start + offset])) for offset in range(block)]


def verify(teacher, prompt_ids: torch.Tensor, candidates: list[int], device: str) -> int:
    """Greedy verification: how many drafted tokens the teacher would itself pick."""
    ids = torch.cat([prompt_ids, torch.tensor([candidates], device=device)], dim=1)
    with torch.no_grad():
        logits = teacher(input_ids=ids).logits
    start = prompt_ids.shape[1] - 1
    accepted = 0
    for offset, candidate in enumerate(candidates):
        if int(torch.argmax(logits[0, start + offset])) != candidate:
            break
        accepted += 1
    return accepted


def measure_acceptance(teacher, student, tokenizer, args, pairs) -> float:
    rng = random.Random(args.seed + 1)
    samples = rng.sample(pairs, min(len(pairs), args.accept_samples))
    accepted = 0
    cycles = 0
    student.eval()
    for _source_lang, target_lang, source_text, _target_text in samples:
        prompt_ids = torch.tensor(
            [tokenizer.encode(prompt_text(source_text, target_lang), add_special_tokens=False)],
            device=args.device,
        )
        candidates = propose(student, tokenizer, prompt_ids, args.block, args.device)
        accepted += verify(teacher, prompt_ids, candidates, args.device)
        cycles += 1
    student.train()
    if not cycles:
        return 0.0
    per_cycle = accepted / cycles
    print(
        f"acceptance: {per_cycle:.2f}/{args.block} tokens per cycle ({cycles} samples) "
        f"-> {(per_cycle + 1):.2f} tokens per draft+verify pair",
        flush=True,
    )
    return per_cycle


def save(student, tokenizer, args, step: int) -> None:
    from safetensors.torch import save_file

    os.makedirs(args.out, exist_ok=True)
    tensors = {name: tensor.detach().cpu() for name, tensor in student.state_dict().items()}
    save_file(tensors, os.path.join(args.out, "student.safetensors"))
    with open(os.path.join(args.out, "student.json"), "w", encoding="utf-8") as handle:
        json.dump(
            {
                "layers": args.student_layers,
                "block": args.block,
                "step": step,
                "model": args.model,
                "tokenizer": getattr(tokenizer, "name_or_path", args.model),
            },
            handle,
            indent=2,
        )
    print(f"saved the student to {args.out} (step {step})", flush=True)


def train(args) -> None:
    teacher, tokenizer = load_model(args.model, args.device)
    student = build_student(teacher, args.student_layers, args.device)
    trainable = [p for p in student.parameters() if p.requires_grad]
    print(f"{sum(p.numel() for p in trainable):,} trainable parameters", flush=True)
    optimizer = torch.optim.AdamW(trainable, lr=args.lr, weight_decay=0.0)
    pairs = load_pairs(args.data, args.pairs)
    print(f"{len(pairs)} pairs from {args.data}", flush=True)
    rng = random.Random(args.seed)
    running = 0.0
    started = time.perf_counter()
    tokens = 0

    for step in range(1, args.steps + 1):
        batch = [rng.choice(pairs) for _ in range(args.batch_size)]
        built = build_batch(batch, tokenizer, args.seq_len, args.block, args.noise_fraction, rng, args.device)
        if built is None:
            continue
        clean_ids, noisy_ids, mask = built
        with torch.no_grad():
            teacher_logits = teacher(input_ids=clean_ids).logits
        student_logits = student(input_ids=noisy_ids).logits
        loss = tv_loss(teacher_logits[mask], student_logits[mask])
        loss.backward()
        if step % args.grad_accum == 0:
            torch.nn.utils.clip_grad_norm_(trainable, 1.0)
            optimizer.step()
            optimizer.zero_grad(set_to_none=True)
        running += float(loss.detach())
        tokens += int(mask.sum())
        if step % args.log_every == 0:
            elapsed = time.perf_counter() - started
            print(
                f"step {step:5d}  loss {running / args.log_every:.4f}  "
                f"{tokens / elapsed:7.0f} loss-positions/s  {elapsed:6.0f}s",
                flush=True,
            )
            running = 0.0
        if step % args.accept_every == 0:
            measure_acceptance(teacher, student, tokenizer, args, pairs)
            save(student, tokenizer, args, step)

    save(student, tokenizer, args, args.steps)
    measure_acceptance(teacher, student, tokenizer, args, pairs)
    # What the mods actually translate: technical prose, not subtitles.
    measure_acceptance(teacher, student, tokenizer, args, list(SYNTHETIC))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="pfnet/plamo-3-translate-0.8b")
    parser.add_argument("--data", default="synthetic", help="synthetic, or hf:<dataset>")
    parser.add_argument("--pairs", type=int, default=200000)
    parser.add_argument("--out", default="draft-distil")
    parser.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    parser.add_argument("--student-layers", type=int, default=2)
    parser.add_argument("--steps", type=int, default=8000)
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--grad-accum", type=int, default=1)
    parser.add_argument("--seq-len", type=int, default=384)
    parser.add_argument("--block", type=int, default=8)
    parser.add_argument("--noise-fraction", type=float, default=0.5)
    parser.add_argument("--lr", type=float, default=1e-4)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--log-every", type=int, default=25)
    parser.add_argument("--accept-every", type=int, default=500)
    parser.add_argument("--accept-samples", type=int, default=24)
    parser.add_argument("--smoke", action="store_true", help="20 steps on the built-in pairs")
    args = parser.parse_args()
    if args.smoke:
        args.steps = 20
        args.accept_every = 20
        args.accept_samples = 4
        args.seq_len = 128
        args.batch_size = 2
        args.log_every = 10
        args.pairs = 32
    train(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
