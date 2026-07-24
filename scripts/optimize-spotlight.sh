#!/bin/bash
#
# Spotlight の索引対象から、検索する必要のない高頻度更新ディレクトリを除外する。
# Spotlight 自体は有効なままにする。
#
# 使い方:
#   bash scripts/optimize-spotlight.sh apply
#   bash scripts/optimize-spotlight.sh status
#   bash scripts/optimize-spotlight.sh restart  # sudo が必要

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "このスクリプトは macOS 専用です" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOTFILES_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MARKER_NAME=".metadata_never_index"

# キャッシュ、依存物、ビルド生成物だけを対象にする。
# Documents や Downloads、リポジトリのソースコードは除外しない。
EXCLUDED_PATHS=(
  "${HOME}/Library/Caches"
  "${HOME}/Library/Logs"
  "${HOME}/Library/Developer/Xcode/DerivedData"
  "${HOME}/Library/Developer/CoreSimulator"
  "${HOME}/Library/Containers/com.docker.docker/Data"
  "${HOME}/.cache"
  "${HOME}/.cargo/git"
  "${HOME}/.cargo/registry"
  "${HOME}/node_modules"
  "${DOTFILES_ROOT}/node_modules"
  "${DOTFILES_ROOT}/tools/claudex-agent-adapter/target"
  "${DOTFILES_ROOT}/.codex/.tmp"
  "${DOTFILES_ROOT}/.codex/plugins/cache"
  "${DOTFILES_ROOT}/.cursor/extensions"
  "${DOTFILES_ROOT}/.cursor/projects"
  "${DOTFILES_ROOT}/.config/opencode/node_modules"
)

show_process_status() {
  local found=false

  printf '%-7s %10s %7s %7s %12s  %s\n' "PID" "RSS(MiB)" "%MEM" "%CPU" "ELAPSED" "COMMAND"
  while read -r pid rss mem cpu elapsed command; do
    [[ -n "${pid:-}" ]] || continue
    found=true
    awk -v pid="${pid}" -v rss="${rss}" -v mem="${mem}" -v cpu="${cpu}" \
      -v elapsed="${elapsed}" -v command="${command}" \
      'BEGIN { printf "%-7s %10.1f %7s %7s %12s  %s\n", pid, rss / 1024, mem, cpu, elapsed, command }'
  done < <(ps -axo pid=,rss=,%mem=,%cpu=,etime=,comm= | awk '$6 ~ /\/(mds|mds_stores)$/')

  if [[ "${found}" == false ]]; then
    echo "mds / mds_stores は現在動作していません"
  fi
}

show_status() {
  echo "==> Spotlight プロセス"
  show_process_status

  echo
  echo "==> ボリュームの索引状態"
  mdutil -as 2>&1 || true

  echo
  echo "==> 高頻度更新ディレクトリの除外状態"
  local path
  for path in "${EXCLUDED_PATHS[@]}"; do
    if [[ ! -d "${path}" ]]; then
      continue
    fi
    if [[ -e "${path}/${MARKER_NAME}" ]]; then
      printf 'excluded  %s\n' "${path}"
    else
      printf 'indexed   %s\n' "${path}"
    fi
  done
}

apply_exclusions() {
  local path marker applied=0 already=0

  echo "==> Spotlight から高頻度更新ディレクトリを除外"
  for path in "${EXCLUDED_PATHS[@]}"; do
    [[ -d "${path}" ]] || continue
    marker="${path}/${MARKER_NAME}"
    if [[ -e "${marker}" ]]; then
      printf 'already   %s\n' "${path}"
      already=$((already + 1))
      continue
    fi
    : > "${marker}"
    printf 'excluded  %s\n' "${path}"
    applied=$((applied + 1))
  done

  echo
  echo "適用: ${applied}、適用済み: ${already}"
  echo "既存の索引と mds_stores のメモリは徐々に解放されます。"
  echo "すぐにプロセスだけ再起動する場合: bash scripts/optimize-spotlight.sh restart"
}

restart_stores() {
  echo "==> mds_stores を一度だけ再起動"
  echo "Spotlight により自動的に再起動されます。定期実行はしません。"
  sudo /usr/bin/killall mds_stores
  sleep 2
  show_process_status
}

case "${1:-apply}" in
  apply)
    apply_exclusions
    ;;
  status)
    show_status
    ;;
  restart)
    restart_stores
    ;;
  *)
    echo "使い方: $0 {apply|status|restart}" >&2
    exit 2
    ;;
esac
