# Qwen3.8-27B on Google Colab for pi

1. Open `qwen38_27b_vllm.ipynb` in Colab on an A100 runtime and run all cells.
   It serves `Qwen/Qwen3.8-27B` with vLLM (on-load FP8, since bf16 ~56GB exceeds 40GB)
   behind a Cloudflare quick tunnel protected by a random API key.
2. On the Mac run the line the last cell prints:
   `./scripts/colab-qwen-connect.sh <tunnel-url> <api-key>`
   (updates the tunnel URL in `.pi/agent/models.json`, stores the key in the fish universal
   variable `COLAB_QWEN_API_KEY`, and checks `/v1/models`).
3. Restart pi: `pi --provider google-colab --model Qwen3.8-27B`.

The tunnel URL and key change every Colab session; `models.json` shows a diff for the URL.
Set `QUANT = "bitsandbytes"` in the notebook for 4-bit if FP8 runs out of KV memory.

Verified (A100 40GB): FP8 + `--enforce-eager --max-num-seqs 4`; pi created a file and ran it via `pi -p`.
The `colab` CLI can drive this without the browser: `colab new -s qwen --gpu A100`, then `colab exec -s qwen --timeout 150 -f script.py`.
