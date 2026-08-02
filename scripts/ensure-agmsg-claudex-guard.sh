#!/usr/bin/env bash

set -euo pipefail

scripts_dir="${AGMSG_SCRIPTS_DIR:-${HOME}/.agents/skills/agmsg/scripts}"
export AGMSG_SCRIPTS_DIR="$scripts_dir"

python3 - <<'PY'
import os
from pathlib import Path

scripts_dir = Path(os.environ["AGMSG_SCRIPTS_DIR"])
shebang = "#!/usr/bin/env bash\n"

child_marker = "# claudex: provider-backed children do not own agmsg watchers."
child_guard = f'''{child_marker}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''

parent_marker = "# claudex: automatic agmsg Monitor is opt-in for the interactive parent."
parent_guard = f'''{parent_marker}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''

watch_marker = "# claudex: serialize same-session watcher claims."
watch_parent_marker = "# claudex: automatic agmsg Monitor is opt-in for the interactive parent."
watch_parent_guard = f'''{watch_parent_marker}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''
watch_child_marker = "# claudex: provider/noninteractive child watchers are disabled."
watch_child_guard = f'''{watch_child_marker}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''
watch_resume_marker = "# claudex: resumed claudex sessions do not run agmsg watchers."
watch_resume_guard = f'''{watch_resume_marker}
if [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ] && [ -n "${{1:-}}" ]; then
  if ps -axo command= 2>/dev/null \\
    | grep -F -- "claudex-agent-adapter launch" \\
    | grep -F -- "--resume $1" >/dev/null 2>&1; then
    exit 0
  fi
fi
'''
watch_claim = f'''{watch_marker}
CLAIM_DIR="${{PIDFILE}}.claim"
mkdir -p "$RUN_DIR" 2>/dev/null || true
CLAIM_ACQUIRED=0
CLAIM_ATTEMPTS=0
while [ "$CLAIM_ATTEMPTS" -lt 100 ]; do
  if mkdir "$CLAIM_DIR" 2>/dev/null; then
    printf '%s\\n' "$$" > "$CLAIM_DIR/owner"
    CLAIM_ACQUIRED=1
    break
  fi
  claim_owner=$(cat "$CLAIM_DIR/owner" 2>/dev/null || true)
  if [ -z "$claim_owner" ] || ! kill -0 "$claim_owner" 2>/dev/null; then
    rm -rf "$CLAIM_DIR" 2>/dev/null || true
    CLAIM_ATTEMPTS=$((CLAIM_ATTEMPTS + 1))
    continue
  fi
  CLAIM_ATTEMPTS=$((CLAIM_ATTEMPTS + 1))
  sleep 0.01
done
if [ "$CLAIM_ACQUIRED" -ne 1 ]; then
  echo "agmsg watch: timed out claiming watcher slot for $SESSION_ID" >&2
  exit 1
fi
'''

for name in ("session-start.sh", "session-end.sh", "check-inbox.sh", "watch.sh"):
    path = scripts_dir / name
    if not path.is_file():
        continue
    text = path.read_text(encoding="utf-8")
    if shebang not in text:
        raise SystemExit(f"unsupported agmsg hook format: {path}")
    if name == "session-start.sh" and parent_marker not in text:
        text = text.replace(shebang, shebang + "\n" + parent_guard, 1)
    if name == "watch.sh" and watch_parent_marker not in text:
        text = text.replace(shebang, shebang + "\n" + watch_parent_guard, 1)
    if name == "watch.sh" and watch_child_marker not in text:
        text = text.replace(watch_parent_guard, watch_parent_guard + watch_child_guard, 1)
    if name == "watch.sh" and watch_resume_marker not in text:
        text = text.replace(watch_child_guard, watch_child_guard + watch_resume_guard, 1)
    if name != "watch.sh" and child_marker not in text:
        text = text.replace(shebang, shebang + "\n" + child_guard, 1)
    if name == "watch.sh" and watch_marker not in text:
        anchor = 'PIDFILE="$RUN_DIR/watch.$SESSION_ID.pid"\n'
        if anchor not in text:
            raise SystemExit(f"unsupported agmsg watcher format: {path}")
        text = text.replace(anchor, anchor + "\n" + watch_claim, 1)
        release = 'echo $$ > "$PIDFILE"\n'
        if release not in text:
            raise SystemExit(f"unsupported agmsg watcher claim release format: {path}")
        text = text.replace(release, release + 'rm -rf "$CLAIM_DIR" 2>/dev/null || true\n', 1)
    path.write_text(text, encoding="utf-8")
PY
