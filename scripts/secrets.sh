#!/bin/bash
# Keep a secret in Git only in encrypted form (age), readable on the machines that hold the identity.
#   ./scripts/secrets.sh keygen           create ~/.config/age/keys.txt and print its public key
#   ./scripts/secrets.sh encrypt <name>   read JSON on stdin, write secrets/<name>.age
#   ./scripts/secrets.sh decrypt <name>   print the decrypted JSON
# Copy ~/.config/age/keys.txt to another Mac (never into Git) and add that Mac's public key to
# secrets/recipients.txt, then re-encrypt every file so both machines can read them.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="${ROOT}/secrets"
IDENTITY="${AGE_IDENTITY:-${HOME}/.config/age/keys.txt}"
RECIPIENTS="${DIR}/recipients.txt"

require() {
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing: ${tool} (brew install age)" >&2; exit 1; }
  done
}

case "${1:-}" in
  keygen)
    require age-keygen
    mkdir -p "$(dirname "${IDENTITY}")"
    if [ -f "${IDENTITY}" ]; then
      echo "present: ${IDENTITY}" >&2
    else
      age-keygen -o "${IDENTITY}" 2>/dev/null
      chmod 600 "${IDENTITY}"
    fi
    age-keygen -y "${IDENTITY}"
    ;;
  encrypt)
    require age
    name="${2:?usage: secrets.sh encrypt <name>}"
    [ -f "${RECIPIENTS}" ] || { echo "missing: ${RECIPIENTS}" >&2; exit 1; }
    mkdir -p "${DIR}"
    tmp="$(mktemp)"
    cat > "${tmp}"
    age -R "${RECIPIENTS}" -o "${DIR}/${name}.age" "${tmp}"
    rm -f "${tmp}"
    echo "wrote: ${DIR}/${name}.age"
    ;;
  decrypt)
    require age
    name="${2:?usage: secrets.sh decrypt <name>}"
    [ -f "${DIR}/${name}.age" ] || { echo "missing: ${DIR}/${name}.age" >&2; exit 1; }
    age -d -i "${IDENTITY}" "${DIR}/${name}.age"
    ;;
  *)
    sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
