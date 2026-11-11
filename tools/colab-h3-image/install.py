# Runs on the Colab VM: install SGLang (main, diffusion extra) and download the weights in parallel.
# Progress: /content/h3/install.log, /content/h3/download.log; markers /content/h3/{installed,downloaded}.
import os, subprocess
os.makedirs("/content/h3", exist_ok=True)
# Colab has no Rust toolchain; SGLang's optional Rust extensions are not needed for diffusion serving.
env = dict(os.environ, HF_XET_HIGH_PERFORMANCE="1", HF_HUB_ENABLE_HF_TRANSFER="0", SGLANG_BUILD_RUST_EXTS="none")
# Some dependencies (outlines_core) have no prebuilt wheel for Colab's Python, so they build from Rust source.
env["PATH"] = f"/root/.cargo/bin:{env['PATH']}"
install = (
    "set -e; cd /content/h3; rm -f installed; "
    "command -v cargo >/dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal; "
    "[ -d sglang ] || git clone --depth 1 https://github.com/sgl-project/sglang.git; "
    "cd sglang && pip install -q -e 'python[diffusion]' && "
    "wget -q https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -O /usr/local/bin/cloudflared && "
    "chmod +x /usr/local/bin/cloudflared && touch /content/h3/installed"
)
download = (
    "set -e; rm -f /content/h3/downloaded; pip install -q -U 'huggingface_hub[hf_xet]' && "
    "hf download MiniMaxAI/MiniMax-H3-Image && "
    "hf download MiniMaxAI/MiniMax-H3 --include 'text_encoder/*' && touch /content/h3/downloaded"
)
for name, cmd in (("install", install), ("download", download)):
    subprocess.Popen(["bash", "-c", cmd], env=env, stdout=open(f"/content/h3/{name}.log", "w"),
                     stderr=subprocess.STDOUT, start_new_session=True)
print("started")
