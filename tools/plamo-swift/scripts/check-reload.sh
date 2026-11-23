#!/bin/bash
# One focused number: how long the lukewarm -> warm reload takes in each server, on spare ports.
cd "$(dirname "$0")/.." || exit 1
B=$PWD/.build/release/plamo-swift
I="$HOME/.local/share/plamo-swift/plamo-swift"
M="$HOME/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
BODY='<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>こんにちは、世界。<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>'
# A different prompt for the reload: the same one would come straight out of the completion cache.
BODY2='<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>今日はいい天気ですね。<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>'
PLAMO_TIMING=1 "$B" serve --model "$M" --port 18558 --idle-seconds 300 --lukewarm-seconds 5 \
  >/tmp/packed-serve.log 2>&1 &
NEW=$!
PLAMO_TIMING=1 "$I" serve --model "$M" --port 18559 --idle-seconds 300 --lukewarm-seconds 5 \
  >/tmp/installed-serve.log 2>&1 &
OLD=$!
trap 'kill $NEW $OLD 2>/dev/null' EXIT
for _ in $(seq 1 120); do
  curl -sf -m 2 -o /dev/null http://127.0.0.1:18558/v1/models || { sleep 0.5; continue; }
  curl -sf -m 2 -o /dev/null http://127.0.0.1:18559/v1/models && break
  sleep 0.5
done
req() {
  python3 - "$1" "$2" <<'PY'
import json, sys, urllib.request
port, body = sys.argv[1], sys.argv[2]
payload = json.dumps({"model": "plamo", "max_tokens": 24,
                      "messages": [{"role": "user", "content": body}]}).encode()
request = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", payload,
                                 {"content-type": "application/json"})
print(json.load(urllib.request.urlopen(request, timeout=120))["choices"][0]["message"]["content"])
PY
}
echo "first : $(req 18558 "$BODY")"
echo "first : $(req 18559 "$BODY")"
for _ in $(seq 1 60); do grep -q 'lukewarm$' /tmp/packed-serve.log && grep -q 'lukewarm$' /tmp/installed-serve.log && break; sleep 0.5; done
echo "--- footprint while lukewarm (new)"
footprint -p "$NEW" 2>/dev/null | grep -E 'phys_footprint:' | head -1
echo "reload: $(req 18558 "$BODY2")"
echo "reload: $(req 18559 "$BODY2")"
sleep 3
echo "--- new (packed) marks"
grep -E 'reload\.|lukewarm\.reload|gen\.' /tmp/packed-serve.log | tail -6
echo "--- installed (loadArrays) marks"
grep -E 'reload\.|lukewarm\.reload|gen\.' /tmp/installed-serve.log | tail -6
