# Runs on the Colab VM: install SGLang (main, diffusion extra) and download the weights in parallel.
# Progress: /content/h3/install.log, /content/h3/download.log; markers /content/h3/{installed,downloaded}.
import os, subprocess

os.makedirs("/content/h3", exist_ok=True)
# Colab has no Rust toolchain; SGLang's optional Rust extensions are not needed for diffusion serving.
# Xet (CAS) downloads hit HTTP 429 on these VMs, so the plain HTTP path is used.
env = dict(os.environ, HF_XET_HIGH_PERFORMANCE="1", HF_HUB_ENABLE_HF_TRANSFER="0",
           HF_HUB_DISABLE_XET="1", SGLANG_BUILD_RUST_EXTS="none")
# Some dependencies (outlines_core) have no prebuilt wheel for Colab's Python, so they build from Rust source.
env["PATH"] = f"/root/.cargo/bin:{env['PATH']}"

# The container disk is ~113 GB but the original checkpoint is ~135 GB, so keep the Hugging Face cache
# on /mnt/local-scratch (369 GB) and leave the default cache path as a symlink for SGLang and hf.
scratch = r"""
set -e
mkdir -p /mnt/local-scratch
if [ -d /root/.cache/huggingface ] && [ ! -L /root/.cache/huggingface ]; then
  cp -a /root/.cache/huggingface /mnt/local-scratch/hf
  rm -rf /root/.cache/huggingface
fi
ln -sfn /mnt/local-scratch/hf /root/.cache/huggingface
pip cache purge >/dev/null 2>&1 || true
"""
subprocess.run(["bash", "-c", scratch], check=True, capture_output=True)

install = (
    "set -e; cd /content/h3; rm -f installed; "
    "command -v cargo >/dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal; "
    "[ -d sglang ] || git clone --depth 1 https://github.com/sgl-project/sglang.git; "
    "cd sglang && pip install -q -e 'python[diffusion]' && "
    "wget -q https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -O /usr/local/bin/cloudflared && "
    "chmod +x /usr/local/bin/cloudflared && touch /content/h3/installed"
)
# Only the original checkpoint: the repository also carries a ~204 GB diffusers copy, and the text
# encoder comes from MiniMax-H3. See the model card's "Download" table.
download = (
    "set -e; rm -f /content/h3/downloaded; pip install -q -U 'huggingface_hub[hf_xet]' && "
    "hf download MiniMaxAI/MiniMax-H3-Image model_index.json 'pdd_*' 'processor/*' 'tokenizer/*' "
    "'transformer/*' 'vae/*' && "
    "hf download MiniMaxAI/MiniMax-H3 --include 'text_encoder/*' && touch /content/h3/downloaded"
)
for name, cmd in (("install", install), ("download", download)):
    subprocess.Popen(["bash", "-c", cmd], env=env, stdout=open(f"/content/h3/{name}.log", "w"),
                     stderr=subprocess.STDOUT, start_new_session=True)
print("started")
