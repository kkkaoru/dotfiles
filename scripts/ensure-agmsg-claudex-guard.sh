#!/usr/bin/env bash

set -euo pipefail

scripts_dir="${AGMSG_SCRIPTS_DIR:-${HOME}/.agents/skills/agmsg/scripts}"
export AGMSG_SCRIPTS_DIR="$scripts_dir"

python3 - <<'PY'
import os
import re
import stat
import tempfile
from pathlib import Path


scripts_dir = Path(os.environ["AGMSG_SCRIPTS_DIR"])
shebang = "#!/usr/bin/env bash\n"

child_marker = "# claudex: provider-backed children do not own agmsg watchers."
child_guard = f'''{child_marker}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 ] \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 ] \\
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

inbox_parent_marker = "# claudex: agmsg turn delivery is opt-in for the interactive parent."
inbox_parent_guard = f'''{inbox_parent_marker}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''

watch_parent_guard = f'''{parent_marker}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] \\
  && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ] \\
  && [ "${{CLAUDEX_AGMSG_EXPLICIT:-}}" != 1 ]; then
  exit 0
fi
'''

watch_child_marker = "# claudex: provider/noninteractive child watchers are disabled."
watch_child_guard = f'''{watch_child_marker}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 ] \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 ] \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''

watch_resume_marker = "# claudex: resumed claudex sessions do not run agmsg watchers."
watch_resume_guard = f'''{watch_resume_marker}
if [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ] \\
  && [ "${{CLAUDEX_AGMSG_EXPLICIT:-}}" != 1 ] \\
  && [ -n "${{1:-}}" ]; then
  if ps -axo command= 2>/dev/null \\
    | grep -F -- "claudex-agent-adapter launch" \\
    | grep -F -- "--resume $1" >/dev/null 2>&1; then
    exit 0
  fi
fi
'''

watch_marker = "# claudex: serialize same-session watcher claims."
watch_claim_version = "# claudex: watcher claim schema v2."
watch_claim = f'''{watch_marker}
{watch_claim_version}
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
watch_claim_cleanup = 'rm -rf "$CLAIM_DIR" 2>/dev/null || true\n'


def marked_block_span(text: str, marker: str) -> tuple[int, int] | None:
    """Return the full marked guard block, including its trailing newline.

    The installer has to repair old malformed blocks, so a marker cannot be
    treated as proof that the block is valid. The generated guards contain
    ordinary Bash ``if``/``fi`` blocks; balancing those lines also handles the
    nested ``if`` in the resume guard.
    """

    marker_start = text.find(marker)
    if marker_start < 0:
        return None
    line_start = text.rfind("\n", 0, marker_start) + 1
    marker_line_end = text.find("\n", marker_start)
    if marker_line_end < 0:
        raise SystemExit(f"unsupported marked agmsg guard format: {marker!r}")

    cursor = marker_line_end + 1
    depth = 0
    saw_if = False
    while cursor <= len(text):
        line_end = text.find("\n", cursor)
        if line_end < 0:
            line_end = len(text)
            next_cursor = len(text)
        else:
            next_cursor = line_end + 1
        line = text[cursor:line_end]
        if re.match(r"^\s*if\b", line):
            depth += 1
            saw_if = True
        if re.match(r"^\s*fi\b", line):
            depth -= 1
            if saw_if and depth == 0:
                return line_start, next_cursor
        if next_cursor >= len(text):
            break
        cursor = next_cursor

    raise SystemExit(f"unsupported marked agmsg guard format: {marker!r}")


def remove_marked_blocks(text: str, marker: str) -> str:
    """Remove every existing instance so malformed/duplicate guards migrate."""

    while marker in text:
        span = marked_block_span(text, marker)
        if span is None:
            break
        start, end = span
        text = text[:start] + text[end:]
    return text


def insert_after_shebang(text: str, block: str) -> str:
    if shebang not in text:
        raise SystemExit("unsupported agmsg hook format: missing bash shebang")
    prefix, rest = text.split(shebang, 1)
    # Removing an old block leaves its separator newline behind. Normalize
    # that whitespace before reinserting so repeated repairs are byte-stable.
    rest = rest.lstrip("\n")
    return prefix + shebang + "\n" + block + rest


def remove_watch_claim_blocks(text: str) -> str:
    """Remove stale claim implementations up to their PIDFILE release line.

    Unlike a normal guard, an old claim block may itself be syntactically
    malformed. The PIDFILE release line is the stable boundary immediately
    after the owned claim block, so use it to migrate stale content instead of
    trusting the marker or requiring the old block to parse first.
    """

    release = 'echo $$ > "$PIDFILE"\n'
    while watch_marker in text:
        marker_start = text.find(watch_marker)
        line_start = text.rfind("\n", 0, marker_start) + 1
        release_start = text.find(release, marker_start)
        if release_start < 0:
            raise SystemExit("unsupported agmsg watcher claim format: missing PIDFILE release")
        text = text[:line_start] + text[release_start:]
    return text


def transform_hook(name: str, text: str) -> str:
    """Return a canonical hook without mutating the installed file."""

    if shebang not in text:
        raise SystemExit(f"unsupported agmsg hook format: {scripts_dir / name}")

    if name == "watch.sh":
        for marker in (parent_marker, watch_child_marker, watch_resume_marker):
            text = remove_marked_blocks(text, marker)
        text = insert_after_shebang(
            text,
            watch_parent_guard + watch_child_guard + watch_resume_guard,
        )

        anchor = 'PIDFILE="$RUN_DIR/watch.$SESSION_ID.pid"\n'
        release = 'echo $$ > "$PIDFILE"\n'
        if text.count(anchor) != 1:
            raise SystemExit(f"unsupported agmsg watcher format: {scripts_dir / name}")
        if text.count(release) != 1:
            raise SystemExit(f"unsupported agmsg watcher claim format: {scripts_dir / name}")

        # Always migrate the claim block, even when its marker is present.
        # This replaces stale limits/content and adds the current schema line.
        text = remove_watch_claim_blocks(text)
        text = text.replace(watch_claim_cleanup, "")
        anchor_end = text.find(anchor) + len(anchor)
        text = text[:anchor_end] + "\n" + watch_claim + text[anchor_end:].lstrip("\n")
        text = text.replace(release, release + watch_claim_cleanup, 1)
        return text

    for marker in (child_marker, parent_marker, inbox_parent_marker):
        text = remove_marked_blocks(text, marker)
    if name == "session-start.sh":
        block = child_guard + parent_guard
    elif name == "check-inbox.sh":
        block = child_guard + inbox_parent_guard
    else:
        block = child_guard
    return insert_after_shebang(text, block)


def stage_bytes(path: Path, payload: bytes, mode: int) -> Path:
    """Write a same-directory temporary file with the target's permissions."""

    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary_path = Path(temporary)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary_path, mode)
    except BaseException:
        temporary_path.unlink(missing_ok=True)
        raise
    return temporary_path


targets: list[tuple[str, Path, bytes, bytes, int]] = []

# First read and transform every installed hook in memory. No target is
# written until every format (especially watch.sh's anchor and claim shape)
# has passed validation.
for name in ("session-start.sh", "session-end.sh", "check-inbox.sh", "watch.sh"):
    path = scripts_dir / name
    if not path.is_file():
        continue
    original = path.read_bytes()
    original_text = original.decode("utf-8")
    transformed_text = transform_hook(name, original_text)
    transformed = transformed_text.encode("utf-8")
    mode = stat.S_IMODE(path.stat().st_mode)
    targets.append((name, path, original, transformed, mode))

staged: list[tuple[Path, Path, bytes, int]] = []
backups: list[tuple[Path, Path, bytes, int]] = []
replaced: list[tuple[Path, Path, bytes, int]] = []
try:
    # Stage all changed contents and all rollback copies before replacing any
    # target. This makes validation/staging failures strictly non-mutating.
    for _name, path, original, transformed, mode in targets:
        if transformed == original:
            continue
        staged_path = stage_bytes(path, transformed, mode)
        staged.append((path, staged_path, original, mode))
        backup_path = stage_bytes(path, original, mode)
        backups.append((path, backup_path, original, mode))

    # Commit each same-directory replacement only after the complete batch is
    # staged. If a replacement unexpectedly fails, restore already-replaced
    # files from their preflight backups before surfacing the error.
    for path, staged_path, original, mode in staged:
        backup = next(item for item in backups if item[0] == path)
        os.replace(staged_path, path)
        replaced.append((path, backup[1], original, mode))

except BaseException:
    for path, _backup_path, original, mode in reversed(replaced):
        try:
            restore_path = stage_bytes(path, original, mode)
            os.replace(restore_path, path)
        except BaseException:
            # Preserve the original failure while making a best effort to
            # restore every target. The normal prevalidation path never enters
            # this branch; it exists for unexpected filesystem errors.
            pass
    raise
finally:
    for _path, temporary, _original, _mode in staged + backups:
        temporary.unlink(missing_ok=True)
PY
