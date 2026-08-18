#!/bin/bash
#
# Spotlight の索引対象から、検索する必要のない高頻度更新ディレクトリを除外する。
# Spotlight 自体は有効なままにする。
#
# 使い方:
#   bash scripts/optimize-spotlight.sh apply     # 除外のみ。パスワード不要
#   bash scripts/optimize-spotlight.sh status    # 確認。パスワード不要
#   bash scripts/optimize-spotlight.sh install   # 常駐開始。kill 用 sudoers は最大1回
#   bash scripts/optimize-spotlight.sh watch     # launchd から呼ばれる監視
#   bash scripts/optimize-spotlight.sh fix       # 索引リセット。管理者パスワードは1回
#   bash scripts/optimize-spotlight.sh uninstall
#

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "このスクリプトは macOS 専用です" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOTFILES_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MARKER_NAME=".metadata_never_index"
LABEL="com.kkkaoru.optimize-spotlight"
LAUNCH_DOMAIN="gui/$(id -u)"
PLIST_PATH="${HOME}/Library/LaunchAgents/${LABEL}.plist"
STATE_DIR="${HOME}/Library/Application Support/${LABEL}"
LOG_PATH="${HOME}/Library/Logs/${LABEL}.log"
# sudoers.d はドットを含むファイル名を無視する。
SUDOERS_PATH="/etc/sudoers.d/kkkaoru-mds-stores-watchdog"
SUDOERS_PATH_LEGACY="/etc/sudoers.d/${LABEL}"
# 再索引中は 1.5GiB 超が普通なので、通常上限は高めにする。
THRESHOLD_MIB=4096
EMERGENCY_MIB=8192
INDEXING_WORKER_MIN=3
WATCH_INTERVAL_SEC=300
APPLY_EVERY_SEC=21600
KILL_COOLDOWN_SEC=1800
KILL_CMD="/usr/bin/killall mds_stores"

# キャッシュ、依存物、ビルド生成物だけを対象にする。
# Documents や Downloads、リポジトリのソースコードは除外しない。
EXCLUDED_PATHS=(
  "${HOME}/Library/Caches"
  "${HOME}/Library/Logs"
  "${HOME}/Library/Metadata/CoreSpotlight"
  "${HOME}/Library/Developer/Xcode/DerivedData"
  "${HOME}/Library/Developer/Xcode/iOS DeviceSupport"
  "${HOME}/Library/Developer/Xcode/Archives"
  "${HOME}/Library/Developer/CoreSimulator"
  "${HOME}/Library/Containers/com.docker.docker/Data"
  "${HOME}/Library/Application Support/Cursor/Cache"
  "${HOME}/Library/Application Support/Cursor/CachedData"
  "${HOME}/Library/Application Support/Code/Cache"
  "${HOME}/Library/Application Support/Code/CachedData"
  "${HOME}/.cache"
  "${HOME}/.cargo/git"
  "${HOME}/.cargo/registry"
  "${HOME}/.rustup"
  "${HOME}/.bun"
  "${HOME}/.npm"
  "${HOME}/.colima"
  "${HOME}/.docker"
  "${HOME}/.ctx"
  "${HOME}/node_modules"
  "${DOTFILES_ROOT}/node_modules"
  "${DOTFILES_ROOT}/tools/claudex-agent-adapter/target"
  "${DOTFILES_ROOT}/.codex/.tmp"
  "${DOTFILES_ROOT}/.codex/plugins/cache"
  "${DOTFILES_ROOT}/.cursor/extensions"
  "${DOTFILES_ROOT}/.cursor/projects"
  "${DOTFILES_ROOT}/.config/opencode/node_modules"
)

# Finder から探す必要のないメディア用ボリューム。
# 動画・3D・音声のメタデータ抽出は mds_stores のメモリを膨らませやすい。
EXCLUDED_VOLUMES=(
  "/Volumes/m.2-ssd"
)

# リポジトリ配下の依存物・生成物を再帰的に除外する根。
SCAN_ROOTS=(
  "${HOME}/ghq"
)

CLUTTER_DIR_NAMES=(
  node_modules
  target
  .next
  .nuxt
  .turbo
  .gradle
  __pycache__
  .venv
  DerivedData
  .build
  Pods
  .git
)

QUIET=false

log() {
  printf '%s %s\n' "$(date '+%F %T')" "$*"
}

mark_excluded() {
  local path="$1"
  local marker="${path}/${MARKER_NAME}"

  if [[ -e "${marker}" ]]; then
    if [[ "${QUIET}" != true ]]; then
      printf 'already   %s\n' "${path}"
    fi
    return 1
  fi
  : > "${marker}"
  if [[ "${QUIET}" != true ]]; then
    printf 'excluded  %s\n' "${path}"
  fi
  return 0
}

mds_stores_rss_mib() {
  ps -axo rss=,comm= | awk '$2 ~ /\/mds_stores$/ { s += $1 } END { printf "%d", int((s + 1023) / 1024) }'
}

can_kill_without_password() {
  sudo -n /usr/bin/true >/dev/null 2>&1
}

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
  echo "==> 固定パスの除外状態"
  local path
  for path in "${EXCLUDED_PATHS[@]}" "${EXCLUDED_VOLUMES[@]}"; do
    if [[ ! -d "${path}" ]]; then
      continue
    fi
    if [[ -e "${path}/${MARKER_NAME}" ]]; then
      printf 'excluded  %s\n' "${path}"
    else
      printf 'indexed   %s\n' "${path}"
    fi
  done

  echo
  echo "==> 常駐"
  if launchctl print "${LAUNCH_DOMAIN}/${LABEL}" >/dev/null 2>&1; then
    printf 'loaded    %s\n' "${LAUNCH_DOMAIN}/${LABEL}"
  else
    printf 'missing   %s\n' "${LAUNCH_DOMAIN}/${LABEL}"
  fi
  if can_kill_without_password; then
    echo "kill      passwordless sudo が使えます"
  else
    echo "kill      passwordless sudo 未設定。install が必要です"
  fi
}

apply_static_exclusions() {
  local path applied=0 already=0

  if [[ "${QUIET}" != true ]]; then
    echo "==> Spotlight から高頻度更新ディレクトリを除外"
  fi
  for path in "${EXCLUDED_PATHS[@]}" "${EXCLUDED_VOLUMES[@]}"; do
    [[ -d "${path}" ]] || continue
    if mark_excluded "${path}"; then
      applied=$((applied + 1))
    else
      already=$((already + 1))
    fi
  done

  log "固定パス 適用: ${applied}、適用済み: ${already}"
}

apply_scan_exclusions() {
  local root name path applied=0 already=0
  local find_expr=()
  local first=true

  if [[ "${QUIET}" != true ]]; then
    echo "==> リポジトリ配下の依存物・生成物を除外"
  fi
  for name in "${CLUTTER_DIR_NAMES[@]}"; do
    if [[ "${first}" == true ]]; then
      find_expr+=( -name "${name}" )
      first=false
    else
      find_expr+=( -o -name "${name}" )
    fi
  done

  for root in "${SCAN_ROOTS[@]}"; do
    [[ -d "${root}" ]] || continue
    while IFS= read -r -d '' path; do
      if [[ -e "${path}/${MARKER_NAME}" ]]; then
        already=$((already + 1))
        continue
      fi
      : > "${path}/${MARKER_NAME}"
      applied=$((applied + 1))
    done < <(find "${root}" -type d \( "${find_expr[@]}" \) -prune -print0 2>/dev/null)
    if [[ "${QUIET}" != true ]]; then
      printf 'root      %s\n' "${root}"
    fi
  done

  log "走査 適用: ${applied}、適用済み: ${already}"
}

apply_exclusions() {
  apply_static_exclusions
  echo
  apply_scan_exclusions
}

write_privileged_script() {
  local tmp_path="$1"
  local volume

  {
    echo '#!/bin/bash'
    echo 'set -u'
    echo 'echo "==> Spotlight 索引を削除して再構築する"'
    echo '/usr/bin/mdutil -X / || true'
    echo '/usr/bin/mdutil -X /System/Volumes/Preboot || true'
    echo '/usr/bin/mdutil -X /System/Volumes/Data || true'
    for volume in "${EXCLUDED_VOLUMES[@]}"; do
      printf '/usr/bin/mdutil -i off %q || true\n' "${volume}"
      printf '/usr/bin/mdutil -E %q || true\n' "${volume}"
    done
    echo "${KILL_CMD} 2>/dev/null || true"
    echo 'echo "==> 管理者作業が完了しました"'
  } > "${tmp_path}"
  chmod 700 "${tmp_path}"
}

run_as_admin() {
  local script_path="$1"

  if sudo -n true 2>/dev/null; then
    sudo /bin/bash "${script_path}"
  elif [[ -t 0 ]]; then
    sudo /bin/bash "${script_path}"
  else
    osascript -e "do shell script \"/bin/bash ${script_path}\" with administrator privileges"
  fi
}

run_privileged_fix() {
  local tmp_path rc=0

  tmp_path="$(mktemp /tmp/optimize-spotlight.XXXXXX)"
  write_privileged_script "${tmp_path}"

  echo "==> 管理者作業を1回の認証で実行します"
  echo "検索は再索引が終わるまで不完全です。除外済みパスは再索引されません。"

  run_as_admin "${tmp_path}" || rc=$?
  rm -f "${tmp_path}"
  return "${rc}"
}

fix_all() {
  apply_exclusions
  echo
  run_privileged_fix
  echo
  sleep 2
  show_process_status
}

file_age_sec() {
  local path="$1"
  if [[ ! -f "${path}" ]]; then
    echo "${APPLY_EVERY_SEC}"
    return
  fi
  awk -v now="$(date +%s)" -v mtime="$(stat -f %m "${path}")" 'BEGIN { print now - mtime }'
}

mdworker_count() {
  ps -axo comm= | awk 'BEGIN { n = 0 } $0 ~ /\/mdworker_shared$/ { n++ } END { print n }'
}

cap_mds_stores_memory() {
  local rss_mib last_kill_age workers
  rss_mib="$(mds_stores_rss_mib)"
  workers="$(mdworker_count)"

  if (( rss_mib < THRESHOLD_MIB )); then
    return 0
  fi

  if (( workers >= INDEXING_WORKER_MIN && rss_mib < EMERGENCY_MIB )); then
    log "mds_stores ${rss_mib}MiB は上限超過ですが、mdworker_shared が ${workers} 個のため再索引中とみなして再起動しません"
    return 0
  fi

  last_kill_age="$(file_age_sec "${STATE_DIR}/last-kill")"
  if (( last_kill_age < KILL_COOLDOWN_SEC )); then
    log "mds_stores ${rss_mib}MiB が上限 ${THRESHOLD_MIB}MiB を超えていますが、再起動クールダウン中です"
    return 0
  fi

  if ! can_kill_without_password; then
    log "mds_stores ${rss_mib}MiB が上限超過ですが、passwordless sudo がないため再起動できません"
    return 0
  fi

  sudo -n ${KILL_CMD} >/dev/null 2>&1 || true
  date +%s > "${STATE_DIR}/last-kill"
  log "mds_stores ${rss_mib}MiB を再起動しました (workers=${workers})"
}

maybe_apply_exclusions() {
  local age
  age="$(file_age_sec "${STATE_DIR}/last-apply")"
  if (( age < APPLY_EVERY_SEC )); then
    return 0
  fi
  QUIET=true
  apply_static_exclusions
  apply_scan_exclusions
  date +%s > "${STATE_DIR}/last-apply"
}

watch_once() {
  mkdir -p "${STATE_DIR}" "$(dirname "${LOG_PATH}")"
  cap_mds_stores_memory
  maybe_apply_exclusions
}

write_launch_agent_plist() {
  mkdir -p "$(dirname "${PLIST_PATH}")" "$(dirname "${LOG_PATH}")"
  cat > "${PLIST_PATH}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>${SCRIPT_DIR}/optimize-spotlight.sh</string>
    <string>watch</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>${WATCH_INTERVAL_SEC}</integer>
  <key>Nice</key>
  <integer>15</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>LowPriorityIO</key>
  <true/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>${HOME}</string>
    <key>PATH</key>
    <string>/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>StandardOutPath</key>
  <string>${LOG_PATH}</string>
  <key>StandardErrorPath</key>
  <string>${LOG_PATH}</string>
</dict>
</plist>
EOF
  plutil -lint "${PLIST_PATH}" >/dev/null
}

install_launch_agent() {
  write_launch_agent_plist
  launchctl bootout "${LAUNCH_DOMAIN}/${LABEL}" >/dev/null 2>&1 || true
  launchctl bootstrap "${LAUNCH_DOMAIN}" "${PLIST_PATH}"
  launchctl enable "${LAUNCH_DOMAIN}/${LABEL}"
  launchctl kickstart -k "${LAUNCH_DOMAIN}/${LABEL}"
  log "LaunchAgent ${LABEL} を有効化しました"
}

write_sudoers_script() {
  local tmp_path="$1"
  local user
  user="$(id -un)"

  cat > "${tmp_path}" <<EOF
#!/bin/bash
set -euo pipefail
umask 077
tmp="\$(mktemp /tmp/kkkaoru-mds-stores-watchdog.XXXXXX)"
printf '%s ALL=(root) NOPASSWD: ${KILL_CMD}, /usr/bin/true\\n' $(printf %q "${user}") > "\${tmp}"
chmod 0440 "\${tmp}"
/usr/sbin/visudo -cf "\${tmp}"
/bin/mv "\${tmp}" ${SUDOERS_PATH}
/bin/chmod 0440 ${SUDOERS_PATH}
/usr/sbin/chown root:wheel ${SUDOERS_PATH}
/bin/rm -f ${SUDOERS_PATH_LEGACY}
EOF
  chmod 700 "${tmp_path}"
}

install_passwordless_kill() {
  local tmp_path rc=0

  if can_kill_without_password; then
    log "passwordless sudo は設定済みです"
    return 0
  fi

  echo "==> mds_stores 再起動用の passwordless sudo を1回だけ設定します"
  tmp_path="$(mktemp /tmp/optimize-spotlight.XXXXXX)"
  write_sudoers_script "${tmp_path}"
  run_as_admin "${tmp_path}" || rc=$?
  rm -f "${tmp_path}"
  if (( rc != 0 )); then
    echo "sudoers の設定に失敗しました。除外の定期適用は動きますが、メモリ上限での再起動はできません。" >&2
    return "${rc}"
  fi
  log "passwordless sudo を設定しました"
}

install_all() {
  mkdir -p "${STATE_DIR}"
  apply_exclusions
  date +%s > "${STATE_DIR}/last-apply"
  echo
  install_launch_agent
  echo
  install_passwordless_kill
  echo
  echo "5分ごとにメモリを確認し、6時間ごとに除外を再適用します。"
  echo "mds_stores が ${THRESHOLD_MIB}MiB を超え、かつ再索引中でなければ自動再起動します。"
}

uninstall_all() {
  launchctl bootout "${LAUNCH_DOMAIN}/${LABEL}" >/dev/null 2>&1 || true
  rm -f "${PLIST_PATH}"
  if [[ -f "${SUDOERS_PATH}" || -f "${SUDOERS_PATH_LEGACY}" ]]; then
    local tmp_path
    tmp_path="$(mktemp /tmp/optimize-spotlight.XXXXXX)"
    cat > "${tmp_path}" <<EOF
#!/bin/bash
/bin/rm -f ${SUDOERS_PATH} ${SUDOERS_PATH_LEGACY}
EOF
    chmod 700 "${tmp_path}"
    run_as_admin "${tmp_path}" || true
    rm -f "${tmp_path}"
  fi
  log "常駐を解除しました"
}

case "${1:-apply}" in
  apply)
    apply_exclusions
    ;;
  status)
    show_status
    ;;
  watch)
    watch_once
    ;;
  install)
    install_all
    ;;
  uninstall)
    uninstall_all
    ;;
  fix)
    fix_all
    ;;
  restart|volumes-off|reset-index)
    echo "管理者パスワードを分割しないため、索引リセットは fix、常駐は install に統合しました。" >&2
    echo "使い方: $0 {apply|status|install|watch|fix|uninstall}" >&2
    exit 2
    ;;
  *)
    echo "使い方: $0 {apply|status|install|watch|fix|uninstall}" >&2
    exit 2
    ;;
esac
