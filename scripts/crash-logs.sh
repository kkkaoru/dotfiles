#!/usr/bin/env bash
# Read macOS's low-level records of a crash, so a cause is identifiable from the terminal:
#   ./scripts/crash-logs.sh list [n]              newest reports: time, process, exception
#   ./scripts/crash-logs.sh show [n|pattern]      one report in full: exception, faulting frames
#   ./scripts/crash-logs.sh log <process> [mins]  the unified log that process wrote before dying
#   ./scripts/crash-logs.sh tail                  follow the unified log for crash-level messages
# A report is `.ips`: one JSON header line, then a JSON body. `show` prints the exception, the
# termination reason and the faulting thread's stack, and falls back to image+offset when Apple's
# libraries carry no symbols — the address is still what identifies the failing code.
set -euo pipefail

REPORTS="${CRASH_REPORTS_DIR:-$HOME/Library/Logs/DiagnosticReports}"

usage() {
  sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
}

# One line per report: date, process, exception type and signal.
list_reports() {
  local limit="${1:-20}"
  python3 - "$REPORTS" "$limit" <<'PY'
import glob, json, os, sys

directory, limit = sys.argv[1], int(sys.argv[2])
paths = sorted(glob.glob(os.path.join(directory, "*.ips")), key=os.path.getmtime, reverse=True)
if not paths:
    print(f"no crash reports in {directory}")
    raise SystemExit(0)
for path in paths[:limit]:
    head = {}
    try:
        with open(path) as handle:
            head = json.loads(handle.readline() or "{}")
    except (OSError, ValueError):
        pass
    process = head.get("app_name") or os.path.basename(path).rsplit("-", 6)[0]
    when = head.get("timestamp") or ""
    summary = ""
    try:
        with open(path) as handle:
            handle.readline()
            body = json.load(handle)
        exception = body.get("exception") or {}
        summary = f"{exception.get('type', '?')} {exception.get('signal', '')} {exception.get('subtype', '')}".strip()
        if not summary:
            summary = (body.get("termination") or {}).get("indicator", "")
    except (OSError, ValueError):
        summary = "(unreadable body)"
    print(f"{when:<24} {process:<28} {summary}")
PY
}

# The newest report, or the newest whose name matches the pattern, in full.
show_report() {
  local selector="${1:-1}"
  python3 - "$REPORTS" "$selector" <<'PY'
import glob, json, os, sys

directory, selector = sys.argv[1], sys.argv[2]
paths = sorted(glob.glob(os.path.join(directory, "*.ips")), key=os.path.getmtime, reverse=True)
if selector.isdigit():
    index = int(selector) - 1
    if index >= len(paths):
        raise SystemExit(f"no report number {selector} in {directory}")
    path = paths[index]
else:
    matches = [p for p in paths if selector in os.path.basename(p)]
    if not matches:
        raise SystemExit(f"no report matching {selector!r} in {directory}")
    path = matches[0]

with open(path) as handle:
    head = json.loads(handle.readline() or "{}")
    body = json.load(handle)

print(f"file         {path}")
print(f"process      {head.get('app_name', '?')} ({head.get('pid', '?')})")
print(f"version      {head.get('app_version', '?')} build {head.get('build_version', '?')}")
print(f"when         {head.get('timestamp', '?')}")
print(f"os           {head.get('os_version', '?')} ({body.get('captureTime', '?')})")
exception = body.get("exception") or {}
print(f"exception    {exception.get('type', '?')} {exception.get('signal', '')} {exception.get('subtype', '')}".rstrip())
codes = exception.get("codes") or ""
if codes:
    print(f"codes        {codes}")
termination = body.get("termination") or {}
for key in ("indicator", "reason", "namespace"):
    if termination.get(key):
        print(f"termination  {key}: {termination[key]}")
images = body.get("usedImages") or []
for note in (body.get("vmSummary"), body.get("asi")):
    if note:
        print(f"note         {note}")

def image_of(frame):
    index = frame.get("imageIndex")
    if isinstance(index, int) and 0 <= index < len(images):
        return images[index]
    return {}

def frames(thread):
    for frame in thread.get("frames", []):
        image = image_of(frame)
        symbol = frame.get("symbol") or image.get("name", "?")
        offset = frame.get("imageOffset", 0)
        if frame.get("symbolLocation") is not None:
            print(f"    {symbol} + {frame['symbolLocation']}")
        else:
            print(f"    {image.get('name', '?')} + {offset}  ({symbol})")

threads = body.get("threads") or []
faulting = body.get("faultingThread", 0)
if threads:
    print(f"\nfaulting thread {faulting}  {threads[faulting].get('name') or threads[faulting].get('queue') or ''}".rstrip())
    frames(threads[faulting])
    print("\nother threads")
    for index, thread in enumerate(threads):
        if index == faulting:
            continue
        top = thread.get("frames") or [{}]
        name = top[0].get("symbol") or image_of(top[0]).get("name", "?")
        print(f"  {index:>3} {thread.get('name') or thread.get('queue') or '':<28} {name}")
else:
    print("\nno thread list in this report")
PY
}

case "${1:-}" in
  list) list_reports "${2:-20}" ;;
  show) show_report "${2:-1}" ;;
  log)
    [ $# -ge 2 ] || usage
    process="$2"
    minutes="${3:-10}"
    log show --last "${minutes}m" --info --debug \
      --predicate "process == \"$process\" AND (messageType == error OR messageType == fault)" 2>&1 |
      tail -60
    ;;
  tail)
    log stream --level debug \
      --predicate 'processImagePath CONTAINS "plamo" OR senderImagePath CONTAINS "CoreML" OR senderImagePath CONTAINS "Espresso"' |
      head -200
    ;;
  *) usage ;;
esac
