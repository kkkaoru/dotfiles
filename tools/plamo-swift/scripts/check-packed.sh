#!/bin/bash
# Parity and lukewarm reload for the packed cache: the new binary on 18558, the installed one on
# 18559, the same templated prompt to both, then the reload that follows the lukewarm gap.
cd "$(dirname "$0")/.." || exit 1
B=$PWD/.build/release/plamo-swift
I="$HOME/.local/share/plamo-swift/plamo-swift"
M="$HOME/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
BODY='<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>こんにちは、世界。<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>'
PLAMO_TIMING=1 "$B" serve --model "$M" --port 18558 --idle-seconds 300 --lukewarm-seconds 5 \
  >/tmp/packed-serve.log 2>&1 &
NEW=$!
PLAMO_TIMING=1 "$I" serve --model "$M" --port 18559 --idle-seconds 300 --lukewarm-seconds 5 \
  >/tmp/installed-serve.log 2>&1 &
OLD=$!
for _ in $(seq 1 120); do
  curl -sf -m 2 -o /dev/null http://127.0.0.1:18558/v1/models || { sleep 0.5; continue; }
  curl -sf -m 2 -o /dev/null http://127.0.0.1:18559/v1/models && break
  sleep 0.5
done
req() {
  python3 - "$1" "$BODY" <<'PY'
import json, sys, urllib.request
port, body = sys.argv[1], sys.argv[2]
payload = json.dumps({"model": "plamo", "max_tokens": 24,
                      "messages": [{"role": "user", "content": body}]}).encode()
request = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", payload,
                                 {"content-type": "application/json"})
print(json.load(urllib.request.urlopen(request, timeout=90))["choices"][0]["message"]["content"])
PY
}
A=$(req 18558)
Bt=$(req 18559)
printf 'new      : %s\ninstalled: %s\n' "$A" "$Bt"
[ "$A" = "$Bt" ] && echo SAME || echo DIFFERENT
for _ in $(seq 1 40); do grep -q 'lukewarm$' /tmp/packed-serve.log && break; sleep 0.5; done
for _ in $(seq 1 40); do grep -q 'lukewarm$' /tmp/installed-serve.log && break; sleep 0.5; done
echo "--- footprint while lukewarm (new, pid $NEW)"
footprint -p "$NEW" 2>/dev/null | grep -E 'phys_footprint:' | head -1
ps -o rss= -p "$NEW" | awk '{printf "rss %.1f MB\n", $1/1024}'
req 18558 >/dev/null
req 18559 >/dev/null
sleep 1
echo "--- reload after the gap: new (packed)"
grep -E 'reload\.|lukewarm\.reload' /tmp/packed-serve.log | tail -4
echo "--- reload after the gap: installed (loadArrays)"
grep -E 'reload\.|lukewarm\.reload' /tmp/installed-serve.log | tail -4
kill "$NEW" "$OLD" 2>/dev/null
wait 2>/dev/null
echo "done"
