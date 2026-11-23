#!/bin/bash

# Give a Windows machine the tmux that pi's detached jobs need
# (tools/pi-tmux-timeout-extension): pi starts tmux itself, so `tmux`, a POSIX `sh` and GNU
# coreutils must be on the Windows PATH, not only inside Git Bash. Installs MSYS2 under
# ~/msys64 and appends Git's and MSYS2's usr/bin to the user PATH, Git's first so `sh` and the
# coreutils stay Git Bash's. Safe to run again.
#   (Git Bash)  ./scripts/setup-tmux-win.sh

set -euo pipefail

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) ;;
  *)
    echo "this script is for Windows; run it from Git Bash" >&2
    exit 1
    ;;
esac

ROOT="${HOME}/msys64"
MSYS2_BIN="${ROOT}/usr/bin"
ARCHIVE="${HOME}/tmp/msys2-base.tar.xz"
URL="https://github.com/msys2/msys2-installer/releases/latest/download/msys2-base-x86_64-latest.tar.xz"
SEVEN_ZIP="/c/Program Files/7-Zip/7z.exe"

# MSYS2's own shell with MSYS2's PATH, so Git's tools cannot leak into pacman.
msys() {
  env -u PATH LC_ALL=C "${MSYS2_BIN}/bash.exe" -lc "$1"
}

install_msys2() {
  if [ -x "${MSYS2_BIN}/bash.exe" ]; then
    echo "present: ${ROOT}"
    return
  fi
  if [ ! -x "${SEVEN_ZIP}" ]; then
    echo "missing: 7-Zip (${SEVEN_ZIP}) decompresses the MSYS2 archive; install it first" >&2
    exit 1
  fi
  mkdir -p "$(dirname "${ARCHIVE}")"
  [ -f "${ARCHIVE}" ] || curl -L --fail -o "${ARCHIVE}" "${URL}"
  # Windows' bsdtar cannot decompress .xz, and 7-Zip drops MSYS2's symlinks when it extracts
  # the tar (pacman-key disappears). Decompress with 7-Zip, then extract with GNU tar.
  [ -f "${ARCHIVE%.xz}" ] || "${SEVEN_ZIP}" x -y "${ARCHIVE}" -o"$(cygpath -w "$(dirname "${ARCHIVE}")")" >/dev/null
  tar -xf "${ARCHIVE%.xz}" -C "${HOME}"
  echo "installed: ${ROOT}"
}

install_tmux() {
  # The first shell runs /etc/post-install, which initialises the pacman keyring.
  msys 'true' >/dev/null 2>&1 || true
  msys 'pacman -Syu --noconfirm --needed' >/dev/null
  msys 'pacman -S --noconfirm --needed tmux'
  "${MSYS2_BIN}/tmux.exe" -V
}

add_to_user_path() {
  local script
  script="$(mktemp -t pi-tmux-path).ps1"
  cat >"${script}" <<PS
\$add = @('$(cygpath -w /usr/bin)', '$(cygpath -w "${MSYS2_BIN}")')
\$path = [Environment]::GetEnvironmentVariable('Path', 'User')
\$changed = \$false
foreach (\$dir in \$add) {
  if (\$path -notlike "*\$dir*") { \$path = \$path.TrimEnd(';') + ';' + \$dir; \$changed = \$true }
}
if (\$changed) { [Environment]::SetEnvironmentVariable('Path', \$path, 'User'); 'user PATH updated' }
else { 'user PATH already has both entries' }
PS
  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "${script}")"
  rm -f "${script}"
}

# One real session the way the extension starts it: tmux hands `sh` a Windows path.
check() {
  local dir="${HOME}/tmp/pi-tmux-check"
  local sock="pi-tmux-setup-check"
  local session="pi-tmux-setup-check-1"
  local out
  mkdir -p "${dir}"
  rm -f "${dir}/out.log"
  out="$(cygpath -w "${dir}")\\out.log"
  PATH="${PATH}:${MSYS2_BIN}"
  export PATH
  tmux -L "${sock}" kill-server 2>/dev/null || true
  tmux -L "${sock}" new-session -d -s "${session}" -- sh -lc "printf TMUX-OK > '${out}'; sleep 2"
  tmux -L "${sock}" has-session -t "${session}" && echo "has-session: ok"
  tmux -L "${sock}" wait-for -S "${sock}-done" && echo "wait-for -S: ok"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -s "${dir}/out.log" ] && break
    sleep 0.5
  done
  tmux -L "${sock}" kill-server 2>/dev/null || true
  if [ "$(cat "${dir}/out.log" 2>/dev/null)" = "TMUX-OK" ]; then
    echo "tmux ran sh with a Windows path: ok"
  else
    echo "tmux could not run sh with a Windows path; see ${dir}" >&2
    return 1
  fi
}

install_msys2
install_tmux
add_to_user_path
check

echo "done: restart the terminal (and pi) so the new Windows PATH reaches new processes"
