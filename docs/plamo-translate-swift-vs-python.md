# Swift server against the Python server, step by step

The same steps on both servers, measured on this Mac in one session: a fresh process per column,
the same ja→en template prompt the mods send, `max_tokens` 32, `temperature` 0,
`stop: ["<|plamo:tag|>", "<|plamo:bos|>"]`, and a distinct nonce in every prompt so the completion
cache cannot answer a row. Spare ports `18601` (Swift) and `18600` (Python), so the launchd service
on `18555` was untouched. Harness: `tools/plamo-swift/scripts/compare-with-python.py`.

M5 Pro, macOS 27.2.1, release `plamo-swift` on the packed cache (`model.packed` beside the weights),
`mlx-lm 0.32.2` with `mlx 0.32.4` for the Python side.

| step | Swift `plamo-swift serve` | Python `plamo-mlx-server` | ratio |
| --- | --- | --- | --- |
| spawn → `/v1/models` answering | 0.062 s | 1.252 s | 20× faster |
| first completion, new text | 0.051 s | 2.583 s | 51× faster |
| steady completion, new text (median of 5) | 0.014 s | 0.020 s | 1.4× |
| next completion after 12 s idle | 0.058 s | 0.041 s | 0.7× |
| physical footprint while warm | 150 MB | 1005 MB | 6.7× smaller |
| physical footprint after the idle gap | 57 MB | 946 MB | 16.6× smaller |

Steady completions were 0.016 / 0.014 / 0.014 / 0.014 / 0.014 s on the Swift side and 0.021 / 0.020
/ 0.020 / 0.021 / 0.019 s on the Python side.

## Reading the rows

**The after-idle row is not a loss.** The Swift process ran with `--lukewarm-seconds 2` and then sat
idle for 12 s, so its 0.058 s *contains* the `mmap` adopt of the packed cache and a fresh prefill.
The Python process released nothing: its 0.041 s is a plain request on a server still holding
946 MB. There is no equal-memory comparison to make, because the Python server has no lukewarm
state — its equivalent of a reload is a restart, i.e. 1.252 s plus 2.583 s.

**Steady is close on purpose.** The answer is a few tokens before the stop tag, so this row is
mostly prefill. 14 ms against 20 ms is the same ballpark the parity work reports for steady-state
decode, and run-to-run noise on this machine is a few milliseconds.

**150 MB warm is the packed cache working.** The 479 MB of weights are file-backed pages adopted
with `MLXArray(rawPointer:)`, not anonymous memory, so `phys_footprint` counts only the tokenizer,
the compiled kernels and the buffers. The pre-packed-cache figure for the same state was 553 MB rss
/ 665 MB phys; the Python server's 1005 MB is anonymous memory it owns.

**The Python cold numbers are page-cache-assisted.** The weights had been read minutes earlier, so
1.252 s and 2.583 s are faster than a true cold disk read. The earlier figures on the same machine
were 1.12 s to `/v1/models` and 2.81 s to the first completion, which is the same magnitude.

**An earlier run of this harness reported 0.001 s steady on both sides.** That was the completion
cache answering, not decode: both servers cache greedy completions, and the Swift one keeps them
for the process lifetime. Only new text measures the servers.

## What is not in the table

- Multi-turn context growth and long prefills; `tools/plamo-swift/README.md` has the prefill and
  per-step decode breakdown at 23, ~500 and 722 tokens.
- Output quality: the parity set (`scripts/check-parity.py`) covers that, and reports equal output.
- Cold disk reads, which need a purge between the two runs.
