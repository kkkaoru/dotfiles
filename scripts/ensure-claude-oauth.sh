#!/usr/bin/env bash

set -euo pipefail

script_path="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")"
exec python3 - "$script_path" "$@" <<'PY'

"""The Python half deliberately keeps credential values inside subprocesses and memory."""
import argparse
import getpass
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

script_path = Path(sys.argv[1]).resolve()
raw_args = sys.argv[2:]

parser = argparse.ArgumentParser(add_help=True)
parser.add_argument("--dry-run", action="store_true")
parser.add_argument("--status", action="store_true")
parser.add_argument("--refresh-within", type=int, default=7200)
parser.add_argument("--no-refresh", action="store_true")
parser.add_argument("--verbose", action="store_true")
parser.add_argument("--install-launch-agent", action="store_true")
parser.add_argument("--uninstall-launch-agent", action="store_true")
args = parser.parse_args(raw_args)

SERVICE = "Claude Code-credentials"
ACCOUNT = os.environ.get("USER") or getpass.getuser()
CONFIG_DIR = Path(os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude"))).expanduser()
CREDENTIALS_FILE = CONFIG_DIR / ".credentials.json"
PLIST_NAME = "com.kkkaoru.ensure-claude-oauth.plist"
PLIST_PATH = Path.home() / "Library" / "LaunchAgents" / PLIST_NAME


def log(message):
    if args.verbose:
        print(message, file=sys.stderr)


def fail(message, code=1):
    print(f"ensure-claude-oauth: {message}", file=sys.stderr)
    raise SystemExit(code)


def run_security(argv, *, capture=False):
    return subprocess.run(
        ["security", *argv],
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
        stderr=subprocess.PIPE if capture else subprocess.DEVNULL,
        text=True,
        check=False,
    )


def discover_services():
    services = {SERVICE}
    result = run_security(["dump-keychain"], capture=True)
    if result.returncode == 0:
        service_re = re.compile(r'"svce"<blob>="(Claude Code-credentials(?:-[^"]+)?)"')
        for line in result.stdout.splitlines():
            service_match = service_re.search(line)
            if service_match:
                services.add(service_match.group(1))
    return sorted(services)


def read_json_blob(blob):
    try:
        value = json.loads(blob)
    except (TypeError, json.JSONDecodeError):
        return None
    if not isinstance(value, dict):
        return None
    oauth = value.get("claudeAiOauth")
    if not isinstance(oauth, dict):
        return None
    return value


def read_file_candidate():
    try:
        return read_json_blob(CREDENTIALS_FILE.read_text(encoding="utf-8"))
    except (OSError, UnicodeError):
        return None


def read_keychain_candidate(service):
    result = run_security(["find-generic-password", "-a", ACCOUNT, "-s", service, "-w"], capture=True)
    if result.returncode != 0:
        return None
    return read_json_blob(result.stdout.strip())


def candidates():
    found = []
    file_value = read_file_candidate()
    if file_value:
        found.append(("credentialsFile", file_value))
    for service in discover_services():
        value = read_keychain_candidate(service)
        if value:
            found.append((f"keychain:{service}", value))
    return found


def oauth_info(value):
    oauth = value.get("claudeAiOauth", {})
    access = str(oauth.get("accessToken") or "")
    refresh = str(oauth.get("refreshToken") or "")
    expires = oauth.get("expiresAt")
    try:
        expires = float(expires) if expires is not None else None
    except (TypeError, ValueError):
        expires = None
    refresh_expires = oauth.get("refreshTokenExpiresAt")
    try:
        refresh_expires = float(refresh_expires) if refresh_expires is not None else None
    except (TypeError, ValueError):
        refresh_expires = None
    now_ms = time.time() * 1000
    refresh_valid = bool(refresh) and (refresh_expires is None or refresh_expires > now_ms)
    eligible = bool(access and refresh) and (not expires or expires > now_ms or refresh_valid)
    return {
        "has_access": bool(access),
        "has_refresh": bool(refresh),
        "expires": expires,
        "refresh_expires": refresh_expires,
        "eligible": eligible,
    }


def choose(found):
    eligible = [(source, value) for source, value in found if oauth_info(value)["eligible"]]
    if not eligible:
        return None
    return max(eligible, key=lambda item: oauth_info(item[1])["expires"] or 0)


def merge_richest(found, winner):
    merged = {}
    for _, value in found:
        for key, item in value.items():
            if key not in merged or (isinstance(item, (dict, list)) and len(json.dumps(item)) > len(json.dumps(merged[key]))):
                merged[key] = item
    merged["claudeAiOauth"] = winner["claudeAiOauth"]
    return merged


def atomic_write(value):
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix=".credentials.", dir=CONFIG_DIR)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(value, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_name, CREDENTIALS_FILE)
        os.chmod(CREDENTIALS_FILE, 0o600)
    finally:
        try:
            os.unlink(temp_name)
        except FileNotFoundError:
            pass


def write_keychain(value):
    payload = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    result = run_security(
        ["add-generic-password", "-U", "-a", ACCOUNT, "-s", SERVICE, "-w", payload]
    )
    if result.returncode != 0:
        run_security(["delete-generic-password", "-a", ACCOUNT, "-s", SERVICE])
        result = run_security(
            ["add-generic-password", "-a", ACCOUNT, "-s", SERVICE, "-w", payload]
        )
    if result.returncode != 0:
        fail("could not update the main Claude Code Keychain item")


def redacted_status(found, winner):
    now_ms = time.time() * 1000
    print("Claude OAuth status:")
    for source, value in found:
        info = oauth_info(value)
        remaining = None if info["expires"] is None else int((info["expires"] - now_ms) / 1000)
        expiry = "unknown" if remaining is None else f"{remaining}s"
        print(f"  {source}: access={info['has_access']} refresh={info['has_refresh']} expires_in={expiry} eligible={info['eligible']}")
    if winner:
        info = oauth_info(winner[1])
        remaining = None if info["expires"] is None else int((info["expires"] - now_ms) / 1000)
        needs = remaining is None or remaining <= args.refresh_within
        file_matches = False
        file_value = read_file_candidate()
        if file_value:
            file_oauth = file_value.get("claudeAiOauth") or {}
            win_oauth = winner[1].get("claudeAiOauth") or {}
            file_matches = (
                file_oauth.get("accessToken") == win_oauth.get("accessToken")
                and file_oauth.get("refreshToken") == win_oauth.get("refreshToken")
            )
        main_value = read_keychain_candidate(SERVICE)
        main_matches = False
        if main_value:
            main_oauth = main_value.get("claudeAiOauth") or {}
            win_oauth = winner[1].get("claudeAiOauth") or {}
            main_matches = (
                main_oauth.get("accessToken") == win_oauth.get("accessToken")
                and main_oauth.get("refreshToken") == win_oauth.get("refreshToken")
            )
        sync_needed = not (file_matches and main_matches)
        print(
            f"  winner={winner[0]} sync_needed={sync_needed} refresh_needed={needs}"
        )
    else:
        print("  winner=none sync_needed=False refresh_needed=False")


def touch_claude():
    search_path = os.pathsep.join(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            os.environ.get("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
        ]
    )
    claude = shutil.which("claude", path=search_path)
    if not claude:
        fail("claude CLI is not installed", 1)
    environment = os.environ.copy()
    environment["PATH"] = search_path
    environment["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"] = "1"
    try:
        result = subprocess.run(
            [claude, "-p", "OK", "--model", "haiku", "--max-turns", "1"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=environment,
            timeout=45,
            check=False,
        )
    except subprocess.TimeoutExpired:
        fail("Claude CLI refresh timed out", 1)
    if result.returncode != 0:
        fail("Claude CLI refresh failed; if this persists run `claude auth login`", 1)


def acquire_lock():
    """Directory lock with stale reclaim. Returns Path or None if another live run holds it."""
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    lock = CONFIG_DIR / ".oauth-ensure.lock"
    stale_after_sec = 300
    for _ in range(2):
        try:
            lock.mkdir(mode=0o700)
            return lock
        except FileExistsError:
            try:
                age = time.time() - lock.stat().st_mtime
            except OSError:
                age = stale_after_sec + 1
            if age > stale_after_sec:
                log(f"removing stale lock ({int(age)}s old)")
                try:
                    lock.rmdir()
                except OSError:
                    shutil.rmtree(lock, ignore_errors=True)
                continue
            return None
    return None


def install_agent():
    PLIST_PATH.parent.mkdir(parents=True, exist_ok=True)
    log_path = Path.home() / "Library" / "Logs" / "ensure-claude-oauth.log"
    path_value = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    content = f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{PLIST_NAME[:-6]}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{script_path}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{path_value}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>3600</integer>
  <key>StandardOutPath</key>
  <string>{log_path}</string>
  <key>StandardErrorPath</key>
  <string>{log_path}</string>
</dict>
</plist>
"""
    PLIST_PATH.write_text(content, encoding="utf-8")
    os.chmod(PLIST_PATH, 0o600)
    label = PLIST_NAME[:-6]
    subprocess.run(
        ["launchctl", "bootout", f"gui/{os.getuid()}/{label}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    result = subprocess.run(
        ["launchctl", "bootstrap", f"gui/{os.getuid()}", str(PLIST_PATH)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode != 0:
        fail(f"could not bootstrap the LaunchAgent: {result.stderr.strip()}", 1)
    print(f"installed {PLIST_PATH}")


def uninstall_agent():
    label = PLIST_NAME[:-6]
    subprocess.run(["launchctl", "bootout", f"gui/{os.getuid()}/{label}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        PLIST_PATH.unlink()
    except FileNotFoundError:
        pass
    print(f"uninstalled {PLIST_PATH}")


if args.install_launch_agent:
    install_agent()
    raise SystemExit(0)
if args.uninstall_launch_agent:
    uninstall_agent()
    raise SystemExit(0)

if args.refresh_within < 0:
    fail("--refresh-within must be non-negative")

found = candidates()
winner = choose(found)
if args.status:
    redacted_status(found, winner)
    raise SystemExit(0 if winner else 2)
if not winner:
    fail("no recoverable credentials; run `claude auth login`", 2)

lock = acquire_lock()
if lock is None:
    # Another ensure is already reconciling stores; treat as success for LaunchAgent.
    log("skipped; another OAuth reconciliation is already running")
    raise SystemExit(0)

try:
    # Re-read under the lock so we do not overwrite a concurrent Claude CLI refresh.
    found = candidates()
    winner = choose(found)
    if not winner:
        fail("no recoverable credentials; run `claude auth login`", 2)
    merged = merge_richest(found, winner[1])
    info = oauth_info(winner[1])
    remaining = None if info["expires"] is None else (info["expires"] - time.time() * 1000) / 1000
    should_refresh = bool(
        info["has_refresh"] and (remaining is None or remaining <= args.refresh_within)
    )
    if not args.dry_run:
        atomic_write(merged)
        write_keychain(merged)
    if should_refresh and not args.no_refresh and not args.dry_run:
        log("refreshing Claude credentials via CLI")
        touch_claude()
        refreshed = candidates()
        refreshed_winner = choose(refreshed)
        if not refreshed_winner:
            fail(
                "Claude CLI refresh produced no recoverable credentials; run `claude auth login`",
                2,
            )
        merged = merge_richest(refreshed, refreshed_winner[1])
        atomic_write(merged)
        write_keychain(merged)
        log("credentials synchronized and refreshed")
    else:
        log(
            "credentials synchronized"
            + ("; refresh not needed" if not should_refresh else "; refresh skipped")
        )
finally:
    try:
        lock.rmdir()
    except OSError:
        pass
PY
