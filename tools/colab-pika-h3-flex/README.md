# Pika H3 Flex on Google Colab

Runs [Pika H3 Flex](https://huggingface.co/Pika-Labs/Pika-H3-Flex) (video + stereo audio, LoRA adapters
on MiniMax H3) on a Colab A100 80GB High-RAM, or an H100 where the account has one, through the official
[`pika-h3-pro` CLI](https://github.com/Pika-Labs/Pika-H3-Flex). This Mac sends text, images, videos and
audio to a small job API on the VM through a Cloudflare quick tunnel and gets the MP4 back.

```bash
tools/colab-pika-h3-flex/up.sh                                  # ~15-20 min on a fresh VM, ~1 min warm
tools/colab-pika-h3-flex/generate.sh "a cat yawning on a sofa" cat.mp4          # t2va
tools/colab-pika-h3-flex/generate.sh -f first.png "the camera pulls back" i2v.mp4  # i2va (first frame)
tools/colab-pika-h3-flex/generate.sh -f a.png -l b.png "a morphs into b" fl.mp4    # fl2va (-l alone: l2va)
tools/colab-pika-h3-flex/generate.sh -r person.png -r dance.mp4 -r song.wav \
  "the person dances to the song" ref.mp4                                         # ref2va
tools/colab-pika-h3-flex/generate.sh -R 768p -a 9:16 -d 10 -s 7 "<prompt>" tall.mp4
tools/colab-pika-h3-flex/generate.sh -k "<prompt>" out.mp4      # keep the VM running afterwards
colab stop -s pika                                              # stop billing by hand (A100 ~8.9 units/hour)
```

`generate.sh` stops the VM (`colab stop -s pika`) as soon as its job is done, succeeded or failed, unless
another job is still queued or running (`GET /v1/videos` reports how many). Pass `-k` when more requests
follow soon: bringing the VM back with `up.sh` takes ~30-40 min. A client killed while it waits stops
nothing, so check `colab sessions` after an interrupted run.

Each result is saved as `<out>.mp4` (24 fps, H.264 + 32 kHz stereo AAC) and `<out>.json` (the CLI's
metadata, including the final prompt). Start with 480p / 5 s: on an A100 80GB that took ~5 min for the
first request and ~2.7 min once the weights were in the page cache (30 s of it sampling, 66.6 GiB peak,
no offload), because every request loads the model again (the CLI is one process per request).

## Inputs

The task follows the inputs: none → `t2va`, `-f` → `i2va`, `-l` → `l2va`, both → `fl2va`, `-r` → `ref2va`.
References keep their order; their kind comes from the file type. `ref2va` takes at most 9 images,
3 videos and 3 audio clips (12 in all, at least one image or video); each video or audio clip is 2-15 s.
Durations are 5-15 s, resolutions 480p or 768p. The CLI checks these and its error comes back as is.
A request body is limited to 100 MB (the quick tunnel's limit).

## How it works

- `install.py`: `pika-h3-pro` from the Pika repository into a Python 3.12 venv (its pinned torch,
  Diffusers and Diffusers patches stay out of Colab's own packages), cloudflared, and the weights the CLI
  every task loads, at the CLI's pinned revisions: MiniMax H3's text encoder and VAEs (~78 GB, straight
  from Hugging Face onto `/mnt/local-scratch`). A task family's transformer and adapter (~66 GB:
  `transformer/` for t2va/i2va/l2va/fl2va, `transformer_ref/` for ref2va) are downloaded by the CLI when
  the first job of that family runs, so that job takes ~10 min longer. Nothing is downloaded to or
  relayed through the Mac.
- `serve.py`: the gateway on `127.0.0.1:18081` and the tunnel. A rerun gets a new tunnel URL and keeps the
  gateway (and its jobs) running.
- `gateway.py`: Bearer-key auth and the job API (`POST /v1/videos`, `GET /v1/videos/<id>`,
  `/content`, `/metadata`). Jobs run one at a time; a run that fails with a CUDA out-of-memory error is
  retried once with `--cpu-offload` (pass `-O` to start with it; it needs ~94 GiB host RAM).
- `up.sh` keeps the key in `~/.config/colab-pika-h3-flex/api_key` (0600), pushes it to the VM as a file,
  and writes `PIKA_API_BASE` / `PIKA_API_KEY` to the repository `.env` (gitignored).

The adapters and MiniMax H3 are under the MiniMax H3 Community License; see the model card and its
NOTICE before publishing or using output commercially.
