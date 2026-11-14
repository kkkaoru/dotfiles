# Runs on the Colab VM after install.py finished: start the job gateway (localhost only) and a Cloudflare
# quick tunnel to it. Prints the tunnel URL. The key file is written by up.sh.
import os, re, subprocess, time

def bg(cmd, log):
    subprocess.Popen(cmd, stdout=open(log, "a"), stderr=subprocess.STDOUT, start_new_session=True)

# Restarting the gateway drops its in-memory job list, so only do it when it is not running.
if subprocess.run(["pgrep", "-f", "/content/pika/gateway.py"], capture_output=True).returncode != 0:
    # The CLI downloads a task family's transformer on first use; Xet (CAS) hits HTTP 429 on these VMs.
    env = dict(os.environ, HF_HUB_DISABLE_XET="1")
    subprocess.Popen(["python3", "/content/pika/gateway.py"], stdout=open("/content/pika/gateway.log", "a"),
                     stderr=subprocess.STDOUT, start_new_session=True, env=env)
    time.sleep(2)
subprocess.run(["pkill", "-f", "cloudflared tunnel"])  # one tunnel only; a rerun gets a fresh URL
time.sleep(1)
open("/content/pika/cf.log", "w").close()
bg(["cloudflared", "tunnel", "--no-autoupdate", "--url", "http://127.0.0.1:18081"], "/content/pika/cf.log")
for _ in range(45):
    time.sleep(2)
    m = re.search(r"https://[a-z0-9-]+\.trycloudflare\.com", open("/content/pika/cf.log").read())
    if m:
        print(m.group(0))
        break
