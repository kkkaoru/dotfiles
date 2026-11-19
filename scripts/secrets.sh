#!/bin/bash
# Keep a secret in Git only in encrypted form (age), and keep the age identity in 1Password or the
# macOS keychain so every Mac of yours can decrypt it without copying a file.
#   ./scripts/secrets.sh keygen            create the identity (if missing), store it, print its key
#   ./scripts/secrets.sh encrypt <name>    read JSON on stdin, write secrets/<name>.age
#   ./scripts/secrets.sh decrypt <name>    print the JSON; cached SECRETS_CACHE_SECONDS (3600)
#   ./scripts/secrets.sh identity          report where the identity is read from
# The identity comes from AGE_IDENTITY (a path), else 1Password (AGE_OP_REF), else the keychain
# (AGE_KEYCHAIN_SERVICE), else ~/.config/age/keys.txt.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="${ROOT}/secrets"
RECIPIENTS="${DIR}/recipients.txt"
IDENTITY_FILE="${AGE_IDENTITY:-${HOME}/.config/age/keys.txt}"
# Every decrypt that reaches 1Password asks for an authorization, so the answer is kept for a while:
# a poller such as scripts/gateway-cost.sh runs every five minutes and would ask each time.
CACHE_DIR="${XDG_CACHE_HOME:-${HOME}/.cache}/dotfiles-secrets"
CACHE_SECONDS="${SECRETS_CACHE_SECONDS:-3600}"
KEYCHAIN_SERVICE="${AGE_KEYCHAIN_SERVICE:-dotfiles-age-identity}"
OP_REF="${AGE_OP_REF:-op://Personal/dotfiles-age-identity/notesPlain}"
OP_VAULT="${AGE_OP_VAULT:-Personal}"
OP_ITEM="${AGE_OP_ITEM:-dotfiles-age-identity}"
TEMP_IDENTITY=""

trap '[ -z "${TEMP_IDENTITY}" ] || rm -f "${TEMP_IDENTITY}"' EXIT

require() {
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing: ${tool} (brew install age)" >&2; exit 1; }
  done
}

op_ready() {
  # `op whoami` fails under desktop-app biometric integration without `op signin`, while `op vault list` succeeds.
  command -v op >/dev/null 2>&1 && { op whoami >/dev/null 2>&1 || op vault list >/dev/null 2>&1; }
}

keychain_ready() {
  command -v security >/dev/null 2>&1 &&
    security find-generic-password -s "${KEYCHAIN_SERVICE}" -w >/dev/null 2>&1
}

# Prints a path holding the identity, taking it from the first store that has it. 1Password and the
# keychain hand age a private temporary copy; the trap above removes it.
resolve_identity() {
  if [ -n "${AGE_IDENTITY:-}" ] && [ -f "${AGE_IDENTITY}" ]; then
    printf '%s\n' "${AGE_IDENTITY}"
    return 0
  fi
  TEMP_IDENTITY="$(mktemp)"
  chmod 600 "${TEMP_IDENTITY}"
  if command -v op >/dev/null 2>&1 && op read "${OP_REF}" >"${TEMP_IDENTITY}" 2>/dev/null; then
    printf '%s\n' "${TEMP_IDENTITY}"
    return 0
  fi
  if security find-generic-password -s "${KEYCHAIN_SERVICE}" -w >"${TEMP_IDENTITY}" 2>/dev/null; then
    printf '%s\n' "${TEMP_IDENTITY}"
    return 0
  fi
  rm -f "${TEMP_IDENTITY}"
  TEMP_IDENTITY=""
  [ -f "${IDENTITY_FILE}" ] || return 1
  printf '%s\n' "${IDENTITY_FILE}"
}

# The identity is useless in a backup nobody restores, so it also lives where your Macs already sync.
# Only the secret line is stored: age accepts it alone, and neither store keeps a comment block intact.
store_identity() {
  local text
  text="$(grep -m1 '^AGE-SECRET-KEY-' "${IDENTITY_FILE}")"
  [ -n "${text}" ] || { echo "no AGE-SECRET-KEY line in ${IDENTITY_FILE}" >&2; return 1; }
  if op_ready; then
    if op item get "${OP_ITEM}" --vault "${OP_VAULT}" >/dev/null 2>&1; then
      op item edit "${OP_ITEM}" --vault "${OP_VAULT}" "notesPlain=${text}" >/dev/null
    else
      op item create --category "Secure Note" --title "${OP_ITEM}" --vault "${OP_VAULT}" \
        "notesPlain=${text}" >/dev/null
    fi
    echo "stored: 1Password ${OP_VAULT}/${OP_ITEM}" >&2
    return 0
  fi
  if command -v security >/dev/null 2>&1; then
    security add-generic-password -U -a "${USER}" -s "${KEYCHAIN_SERVICE}" -w "${text}" >/dev/null
    echo "stored: keychain ${KEYCHAIN_SERVICE}" >&2
    return 0
  fi
  echo "kept only in ${IDENTITY_FILE}: install the 1Password CLI to share it across Macs" >&2
}

case "${1:-}" in
  keygen)
    require age-keygen
    mkdir -p "$(dirname "${IDENTITY_FILE}")"
    if [ -f "${IDENTITY_FILE}" ]; then
      echo "present: ${IDENTITY_FILE}" >&2
    else
      age-keygen -o "${IDENTITY_FILE}" 2>/dev/null
      chmod 600 "${IDENTITY_FILE}"
    fi
    store_identity
    age-keygen -y "${IDENTITY_FILE}"
    ;;
  encrypt)
    require age
    name="${2:?usage: secrets.sh encrypt <name>}"
    [ -f "${RECIPIENTS}" ] || { echo "missing: ${RECIPIENTS}" >&2; exit 1; }
    mkdir -p "${DIR}"
    plain="$(mktemp)"
    trap 'rm -f "${plain}"' EXIT
    cat > "${plain}"
    age -R "${RECIPIENTS}" -o "${DIR}/${name}.age" "${plain}"
    echo "wrote: ${DIR}/${name}.age"
    ;;
  decrypt)
    require age
    name="${2:?usage: secrets.sh decrypt <name>}"
    [ -f "${DIR}/${name}.age" ] || { echo "missing: ${DIR}/${name}.age" >&2; exit 1; }
    cache="${CACHE_DIR}/${name}.json"
    if [ -f "${cache}" ] && [ ! "${DIR}/${name}.age" -nt "${cache}" ] &&
      [ -z "$(find "${cache}" -mmin "+$((CACHE_SECONDS / 60))" 2>/dev/null)" ]; then
      cat "${cache}"
      exit 0
    fi
    path="$(resolve_identity)" || {
      echo "no identity: run secrets.sh keygen, or copy ~/.config/age/keys.txt here" >&2
      exit 1
    }
    plaintext="$(age -d -i "${path}" "${DIR}/${name}.age")"
    mkdir -p "${CACHE_DIR}"
    chmod 700 "${CACHE_DIR}"
    (umask 077; printf '%s\n' "${plaintext}" > "${cache}")
    printf '%s\n' "${plaintext}"
    ;;
  identity)
    if [ -n "${AGE_IDENTITY:-}" ] && [ -f "${AGE_IDENTITY}" ]; then
      echo "file ${AGE_IDENTITY}"
    elif command -v op >/dev/null 2>&1 && op read "${OP_REF}" >/dev/null 2>&1; then
      echo "1Password ${OP_REF}"
    elif keychain_ready; then
      echo "keychain ${KEYCHAIN_SERVICE}"
    elif [ -f "${IDENTITY_FILE}" ]; then
      echo "file ${IDENTITY_FILE}"
    else
      echo none
      exit 1
    fi
    ;;
  *)
    sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
