# Runs on the Colab VM after install.py finished: start SGLang (localhost only), the auth gateway, and a
# Cloudflare quick tunnel to the gateway. Prints the tunnel URL. The key file is written by up.sh.
import glob, re, subprocess, time, urllib.request

def bg(cmd, log):
    subprocess.Popen(cmd, stdout=open(log, "w"), stderr=subprocess.STDOUT, start_new_session=True)

def up(url):
    try:
        urllib.request.urlopen(url, timeout=3)
        return True
    except Exception:
        return False

if not up("http://127.0.0.1:30000/health"):
    # Single 80 GB GPU: SGLang's auto residency keeps component offload below 130 GiB per GPU.
    # The H3-Image snapshot also ships a text_encoder/; SGLang refuses the ambiguity unless one is pinned.
    text_encoder = glob.glob("/root/.cache/huggingface/hub/models--MiniMaxAI--MiniMax-H3/snapshots/*/text_encoder")[0]
    bg(["sglang", "serve", "--model-path", "MiniMaxAI/MiniMax-H3-Image", "--host", "127.0.0.1",
        "--port", "30000", "--warmup-mode", "off", "--text-encoder-path", text_encoder], "/content/h3/sglang.log")
# Always restart the gateway so a re-pushed gateway.py takes effect (SGLang above is kept running).
subprocess.run(["pkill", "-f", "/content/h3/gateway.py"])
time.sleep(1)
bg(["python3", "/content/h3/gateway.py"], "/content/h3/gateway.log")
time.sleep(2)
subprocess.run(["pkill", "-f", "cloudflared tunnel"])  # one tunnel only; a rerun gets a fresh URL
time.sleep(1)
bg(["cloudflared", "tunnel", "--no-autoupdate", "--url", "http://127.0.0.1:18080"], "/content/h3/cf.log")
for _ in range(45):
    time.sleep(2)
    m = re.search(r"https://[a-z0-9-]+\.trycloudflare\.com", open("/content/h3/cf.log").read())
    if m:
        print(m.group(0))
        break
