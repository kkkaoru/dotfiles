#!/usr/bin/env bash
set -euo pipefail

fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# The shared skill lives at .agents/skills/project-skills/scripts.
storage=${AGENT_SKILLS_STORAGE:-"$script_dir/../../../skills-stroage"}
[ -d "$storage" ] || fail "Storage directory not found: $storage"
storage=$(cd -- "$storage" && pwd -P)

case "${1:-}" in
  list)
    [ "$#" -eq 1 ] || fail 'Usage: manage.sh list'
    for source in "$storage"/*; do
      [ -d "$source" ] && [ -f "$source/SKILL.md" ] || continue
      printf '%s\n' "${source##*/}"
    done
    ;;
  add)
    [ "$#" -eq 3 ] || fail 'Usage: manage.sh add /absolute/project skill-id'
    project=$2
    name=$3
    case "$project" in /*) ;; *) fail 'Project path must be absolute';; esac
    case "$name" in ''|*[!a-z0-9-]*|-*|*-|*--*) fail 'Invalid skill ID';; esac
    [ "${#name}" -le 64 ] || fail 'Skill ID is too long'
    source="$storage/$name"
    [ -d "$source" ] && [ -f "$source/SKILL.md" ] || fail "Unknown skill: $name"
    [ ! -L "$source" ] || fail 'Symlinked source is not supported'
    links=$(find "$source" -type l -print -quit)
    [ -z "$links" ] || fail 'Skill contains a symlink; review it manually'
    [ -d "$project" ] || fail 'Project must already exist'
    project=$(cd -- "$project" && pwd -P)
    [ ! -L "$project/.agents" ] || fail 'Project .agents must not be a symlink'
    [ ! -L "$project/.agents/skills" ] || fail 'Project skills directory must not be a symlink'
    # Never accidentally install optional skills back into this shared home.
    [ "$project/.agents" != "$(dirname -- "$storage")" ] || fail 'Refusing shared global skill installation'
    destination="$project/.agents/skills/$name"
    [ ! -e "$destination" ] && [ ! -L "$destination" ] || fail "Destination already exists: $destination"
    mkdir -p -- "$project/.agents/skills"
    mkdir -- "$destination"
    # Only this freshly reserved directory is owned by this operation.
    trap 'rm -rf -- "$destination"' EXIT
    cp -RP -- "$source/." "$destination/"
    trap - EXIT
    printf 'Added %s to %s\n' "$name" "$destination"
    ;;
  *) fail 'Usage: manage.sh list | manage.sh add /absolute/project skill-id';;
esac
