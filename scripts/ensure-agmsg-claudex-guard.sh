#!/usr/bin/env bash

set -euo pipefail

scripts_dir="${AGMSG_SCRIPTS_DIR:-${HOME}/.agents/skills/agmsg/scripts}"
export AGMSG_SCRIPTS_DIR="$scripts_dir"

python3 - <<'PY'
import os
from pathlib import Path

scripts_dir = Path(os.environ["AGMSG_SCRIPTS_DIR"])
marker = "# claudex: provider-backed children do not own agmsg watchers."
guard = f'''{marker}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 ] \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 ] \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''

for name in ("session-start.sh", "session-end.sh", "check-inbox.sh"):
    path = scripts_dir / name
    if not path.is_file():
        continue
    text = path.read_text(encoding="utf-8")
    if marker in text:
        continue
    shebang = "#!/usr/bin/env bash\n"
    if shebang not in text:
        raise SystemExit(f"unsupported agmsg hook format: {path}")
    path.write_text(text.replace(shebang, shebang + "\n" + guard, 1), encoding="utf-8")
PY
