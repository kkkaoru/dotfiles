# Qwen3.8-27B on Google Colab for pi

## Setup on a new machine

```bash
git pull
./scripts/setup-colab-qwen.sh   # colab CLI (uv), extension deps, extension link, API key
colab --auth=oauth2 sessions    # one-time Google sign-in, if the script asks for it
```

The API key is created once in `~/.pi/agent/auth.json` (never in Git). Then pick
`google-colab / Qwen3.8-27B` in pi. Use one machine at a time: each keeps its own idle state, so
two machines would stop each other's Colab.

## How it works

- `pi-colab-lifecycle-extension` starts Colab on demand (`scripts/colab-qwen-up.sh`: A100, vLLM
  with FP8 + `--enforce-eager --max-num-seqs 4`, Cloudflare quick tunnel), points the
  `google-colab` provider at the tunnel URL, and shows the remaining compute units in the footer.
- `scripts/colab-qwen-idle-stop.sh` (detached watcher) stops the session after 10 minutes without
  a request. The first request after a stop takes ~10 min.
- `qwen38_27b_vllm.ipynb` is the same server as a notebook, for running it by hand in the Colab UI.
- bf16 weights (~56GB) do not fit an A100 40GB, hence FP8 on load; eager mode and a small batch
  avoid a CUDA-graph profiling OOM.
