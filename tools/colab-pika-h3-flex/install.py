# Runs on the Colab VM: install pika-h3-pro into a Python 3.12 venv and download the weights in parallel.
# Progress: /content/pika/install.log, /content/pika/download.log; markers /content/pika/{installed,downloaded}.
import os, subprocess

os.makedirs("/content/pika", exist_ok=True)
# Xet (CAS) downloads hit HTTP 429 on these VMs, so the plain HTTP path is used.
env = dict(os.environ, HF_HUB_DISABLE_XET="1", HF_HUB_ENABLE_HF_TRANSFER="0", UV_LINK_MODE="copy")
env["PATH"] = f"/root/.local/bin:{env['PATH']}"

# The container disk is ~113 GB and the weights are ~216 GB, so keep the Hugging Face cache on
# /mnt/local-scratch (369 GB on A100/H100 VMs) behind the default cache path.
scratch = r"""
set -e
mkdir -p /mnt/local-scratch/hf /root/.cache
if [ -d /root/.cache/huggingface ] && [ ! -L /root/.cache/huggingface ]; then
  cp -a /root/.cache/huggingface/. /mnt/local-scratch/hf/
  rm -rf /root/.cache/huggingface
fi
ln -sfn /mnt/local-scratch/hf /root/.cache/huggingface
"""
subprocess.run(["bash", "-c", scratch], check=True, capture_output=True)

# The pinned torch/diffusers stack and the Diffusers patches would fight Colab's own packages, so the
# CLI lives in its own venv. Triton needs a compiler and patch/ffmpeg at runtime (pip install path).
install = (
    "set -e; cd /content/pika; rm -f installed; "
    "apt-get -qq update && apt-get -qq install -y patch ffmpeg build-essential >/dev/null; "
    "command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh; "
    "[ -d Pika-H3-Flex ] || git clone --depth 1 https://github.com/Pika-Labs/Pika-H3-Flex.git; "
    "[ -x venv/bin/python ] || uv venv --python 3.12 venv; "
    # --python: Colab's environment makes uv pip target the system Python even with VIRTUAL_ENV set.
    "cd Pika-H3-Flex && uv pip install --python ../venv/bin/python --index-strategy unsafe-best-match "
    "-r requirements.txt && "
    "../venv/bin/python tools/apply_diffusers_patches.py && "
    "uv pip install --python ../venv/bin/python --no-deps -e . && "
    "wget -q https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -O /usr/local/bin/cloudflared && "
    "chmod +x /usr/local/bin/cloudflared && touch /content/pika/installed"
)
# Only the parts every task loads, at the CLI's pinned revisions (~78 GB). Each task family's transformer
# (~66 GB: transformer/ for t2va/i2va/l2va/fl2va, transformer_ref/ for ref2va) and adapter are fetched by
# the CLI's own snapshot_download the first time a job needs them, so an unused family is never downloaded.
# The base repo's FL2VA/ and Ref2VA/ copies (2 x 144 GB) are never needed.
LICENSES = ("LICENSE", "NOTICE", "licenses/*")
includes = lambda *patterns: " ".join(f"--include '{p}'" for p in patterns)  # one flag per pattern
download = (
    "set -e; rm -f /content/pika/downloaded; pip install -q -U huggingface_hub && "
    "hf download MiniMaxAI/MiniMax-H3 --revision 42ed227ee7df40d41602854ae760620d6eb651fe "
    f"{includes('text_encoder/*', 'tokenizer/*', 'processor/*', 'vae/*', 'audio_vae/*', '*.json', *LICENSES)} "
    "&& touch /content/pika/downloaded"
)
for name, cmd in (("install", install), ("download", download)):
    subprocess.Popen(["bash", "-c", cmd], env=env, stdout=open(f"/content/pika/{name}.log", "w"),
                     stderr=subprocess.STDOUT, start_new_session=True)
print("started")
