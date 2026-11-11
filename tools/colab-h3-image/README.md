# MiniMax-H3-Image on Google Colab

Serves [MiniMaxAI/MiniMax-H3-Image](https://huggingface.co/MiniMaxAI/MiniMax-H3-Image) from a Colab
A100 80GB (High-RAM) as a MiniMax-API-compatible endpoint, reachable through a Cloudflare quick tunnel.

```bash
tools/colab-h3-image/up.sh                    # ~40 min on a fresh VM (135 GB of weights), ~4 min warm
                                              # writes MINIMAX_API_BASE/MINIMAX_API_KEY to the repo .env
tools/colab-h3-image/generate.sh "a cat yawning" out.png 2048x2048
H3_SHARE=1 tools/colab-h3-image/generate.sh ...   # also upload to R2 and print a public URL
scripts/cf-share-image.sh out.png             # share an existing image
colab stop -s h3img                           # stop billing (~8.9 compute units/hour)
```

Both scripts load the dotfiles `.env` (gitignored) through `scripts/lib/dotfiles-env.sh`, so they work
from any working directory. `MINIMAX_API_BASE` / `MINIMAX_API_KEY` can also be exported for a single
run to override it. `cf-share-image.sh` uploads to the public `CF_SHARE_BUCKET` R2 bucket
(default `portfolio`, served from its `r2.dev` domain) and prints the URL.

## API

All POSTs need `Authorization: Bearer $MINIMAX_API_KEY`; anything else gets 401.

| Endpoint | Request | Response |
| --- | --- | --- |
| `POST /v1/images/generations` | MiniMax H3: `prompt`, `size` (`WxH`), `seed`, … | `data[0].b64_json` (PNG) |
| `POST /v1/image_generation` | MiniMax classic: `prompt`, `aspect_ratio` or `size`, `n` (≤4), `seed` | `data.image_base64[]`, `base_resp` |
| `GET /health` | — (no auth) | `{"status":"ok"}` |

Generation takes 2–3 minutes (≈45 s text encoding on a cold prompt, ≈2 min denoising at 2048x2048).
The gateway answers 200 immediately and streams whitespace until the JSON is ready, because the
quick tunnel drops responses without data after 100 s. Errors therefore arrive as HTTP 200 with
`base_resp.status_code != 0`.

## How it works

- `install.py`: Rust toolchain (needed to build `outlines_core` for Python 3.13), SGLang `main` with the
  diffusion extra (`SGLANG_BUILD_RUST_EXTS=none`), cloudflared, and the weights including the
  `MiniMaxAI/MiniMax-H3` text encoder.
- `serve.py`: SGLang on `127.0.0.1:30000` with `--text-encoder-path` pinned (the H3-Image snapshot also
  ships a `text_encoder/`, which SGLang rejects as ambiguous), the gateway on `127.0.0.1:18080`
  (8080 is Colab's own proxy), and the tunnel. Re-running restarts the gateway and tunnel only.
- `gateway.py`: auth and the MiniMax request/response shapes. The key is pushed by `up.sh` as a file
  (`/content/h3/api_key`, 0600), never on a command line or in Git.

H100 is not available on this account; A100 High-RAM (80 GB GPU, ~180 GB RAM) works with SGLang's
automatic component offload. The model is under the MiniMax-H3-Image Community License; see the
model card before any use that Section IV of the license covers.
