#!/usr/bin/env bash
# Manage project-selected skills from the shared skills-stroage library.
# Installations are hard links: project files share inodes with storage, so a
# storage edit reaches every linked project without copying or drift.
set -euo pipefail

fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*"; }

usage() {
  cat <<'USAGE'
Usage:
  manage.sh list
  manage.sh status /absolute/project
  manage.sh add /absolute/project skill-id
  manage.sh update /absolute/project skill-id [--force]
  manage.sh remove /absolute/project skill-id [--force]

  --force  Replace or delete project files that differ from storage, and drop
           project files that storage no longer contains.
USAGE
}

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# The shared skill lives at .agents/skills/project-skills/scripts.
storage=${AGENT_SKILLS_STORAGE:-"$script_dir/../../../skills-stroage"}
[ -d "$storage" ] || fail "Storage directory not found: $storage"
storage=$(cd -- "$storage" && pwd -P)

force=false
if [ "${!#:-}" = "--force" ]; then
  force=true
  set -- "${@:1:$#-1}"
fi
project=
source_of() { printf '%s/%s' "$storage" "$1"; }

files_of() { # directory -> NUL-separated relative file paths
  (cd -- "$1" && find . -mindepth 1 -print0)
}

validate_id() {
  case "$1" in
    '' | *[!a-z0-9-]* | -* | *- | *--*) fail 'Invalid skill ID' ;;
  esac
  [ "${#1}" -le 64 ] || fail 'Skill ID is too long'
}

require_source() { # skill-id -> storage directory
  validate_id "$1"
  local source
  source=$(source_of "$1")
  [ -d "$source" ] && [ -f "$source/SKILL.md" ] || fail "Unknown skill: $1"
  [ ! -L "$source" ] || fail 'Symlinked source is not supported'
  [ -z "$(find "$source" -type l -print -quit)" ] ||
    fail 'Skill contains a symlink; review it manually'
  printf '%s' "$source"
}

require_project() { # project path -> canonical project directory
  case "$1" in /*) ;; *) fail 'Project path must be absolute' ;; esac
  [ -d "$1" ] || fail 'Project must already exist'
  local project
  project=$(cd -- "$1" && pwd -P)
  [ ! -L "$project/.agents" ] || fail 'Project .agents must not be a symlink'
  [ ! -L "$project/.agents/skills" ] || fail 'Project skills directory must not be a symlink'
  # Never accidentally install optional skills back into this shared home.
  [ "$project/.agents" != "$(dirname -- "$storage")" ] ||
    fail 'Refusing shared global skill installation'
  printf '%s' "$project"
}

link_tree() { # source_dir destination_dir
  local source=$1 destination=$2 relative
  while IFS= read -r -d '' relative; do
    relative=${relative#./}
    if [ -d "$source/$relative" ]; then
      mkdir -p -- "$destination/$relative"
    else
      mkdir -p -- "$(dirname -- "$destination/$relative")"
      ln -- "$source/$relative" "$destination/$relative" 2>/dev/null ||
        fail "Cannot hard-link $relative; storage and the project need one filesystem"
    fi
  done < <(files_of "$source")
}

count_tree() { # source_dir destination_dir -> linked identical diverged missing extra
  local source=$1 destination=$2 relative
  local linked=0 identical=0 diverged=0 missing=0 extra=0
  while IFS= read -r -d '' relative; do
    relative=${relative#./}
    [ -d "$source/$relative" ] && continue
    if [ ! -e "$destination/$relative" ]; then
      missing=$((missing + 1))
    elif [ "$source/$relative" -ef "$destination/$relative" ]; then
      linked=$((linked + 1))
    elif cmp -s -- "$source/$relative" "$destination/$relative"; then
      identical=$((identical + 1))
    else
      diverged=$((diverged + 1))
    fi
  done < <(files_of "$source")
  while IFS= read -r -d '' relative; do
    relative=${relative#./}
    [ -d "$destination/$relative" ] && continue
    [ -e "$source/$relative" ] || [ -L "$source/$relative" ] || extra=$((extra + 1))
  done < <(files_of "$destination")
  printf '%s %s %s %s %s' "$linked" "$identical" "$diverged" "$missing" "$extra"
}

describe_state() { # linked identical diverged missing extra -> one line
  local linked=$1 identical=$2 diverged=$3 missing=$4 extra=$5
  if [ "$diverged" -eq 0 ] && [ "$missing" -eq 0 ] && [ "$extra" -eq 0 ]; then
    if [ "$identical" -eq 0 ]; then
      printf 'linked'
    else
      printf 'identical, not hard-linked'
    fi
    return
  fi
  printf 'diverged: %s linked, %s identical, %s changed, %s missing, %s extra' \
    "$linked" "$identical" "$diverged" "$missing" "$extra"
}

list_skills() {
  local source name description
  for source in "$storage"/*; do
    [ -d "$source" ] && [ -f "$source/SKILL.md" ] || continue
    name=${source##*/}
    description=$(sed -n 's/^description:[[:space:]]*//p' "$source/SKILL.md" | head -1)
    case "$description" in '>' | '>-' | '>+' | '|' | '|-' | '|+') description='' ;; esac
    printf '%s\t%s\n' "$name" "$description"
  done
}

print_state() { # skill-id destination
  local name=$1 destination=$2 counts
  if [ ! -d "$(source_of "$name")" ] || [ ! -f "$(source_of "$name")/SKILL.md" ]; then
    note "$name: local (not in storage)"
    return
  fi
  counts=$(count_tree "$(source_of "$name")" "$destination")
  # shellcheck disable=SC2086
  note "$name: $(describe_state $counts)"
}

status_project() { # project
  local project=$1 directory
  local found=false
  if [ -d "$project/.agents/skills" ]; then
    for directory in "$project/.agents/skills"/*; do
      [ -d "$directory" ] && [ ! -L "$directory" ] || continue
      found=true
      print_state "${directory##*/}" "$directory"
    done
  fi
  if [ "$found" = false ]; then
    note "No project skills installed in $project/.agents/skills"
  fi
}

add_skill() { # project skill-id
  local project=$1 name=$2 source destination
  source=$(require_source "$name")
  destination="$project/.agents/skills/$name"
  [ ! -e "$destination" ] && [ ! -L "$destination" ] ||
    fail "Destination already exists: $destination"
  mkdir -p -- "$project/.agents/skills"
  mkdir -- "$destination"
  # Only this freshly reserved directory is owned by this operation.
  trap 'rm -rf -- "$destination"' EXIT
  link_tree "$source" "$destination"
  trap - EXIT
  note "Linked $name into $destination"
}

update_skill() { # project skill-id force
  local project=$1 name=$2 forced=$3
  local source destination relative refused=false
  source=$(require_source "$name")
  destination="$project/.agents/skills/$name"
  [ -d "$destination" ] || fail "Not installed: $destination"
  while IFS= read -r -d '' relative; do
    relative=${relative#./}
    [ -d "$source/$relative" ] && continue
    if [ ! -e "$destination/$relative" ]; then
      mkdir -p -- "$(dirname -- "$destination/$relative")"
      ln -- "$source/$relative" "$destination/$relative" 2>/dev/null ||
        fail "Cannot hard-link $relative; storage and the project need one filesystem"
      note "  + $relative"
    elif [ "$source/$relative" -ef "$destination/$relative" ]; then
      continue
    elif [ "$forced" = true ] || cmp -s -- "$source/$relative" "$destination/$relative"; then
      rm -f -- "$destination/$relative"
      ln -- "$source/$relative" "$destination/$relative"
      note "  ~ $relative"
    else
      note "  ! $relative differs from storage; rerun with --force to replace it"
      refused=true
    fi
  done < <(files_of "$source")
  while IFS= read -r -d '' relative; do
    relative=${relative#./}
    [ -d "$destination/$relative" ] && continue
    [ -e "$source/$relative" ] && continue
    if [ "$forced" = true ]; then
      rm -f -- "$destination/$relative"
      note "  - $relative"
    else
      note "  ! $relative is not in storage; rerun with --force to delete it"
      refused=true
    fi
  done < <(files_of "$destination")
  [ "$refused" = false ] || fail 'Update kept modified project files'
  # Drop directories that storage no longer provides.
  find "$destination" -mindepth 1 -type d -empty -delete
  note "Updated $name in $destination"
}

remove_skill() { # project skill-id force
  local project=$1 name=$2 forced=$3 destination counts
  destination="$project/.agents/skills/$name"
  [ -d "$destination" ] || fail "Not installed: $destination"
  if [ "$forced" = false ]; then
    if [ ! -d "$(source_of "$name")" ]; then
      fail "Cannot verify $name against storage; rerun with --force to delete it"
    fi
    counts=$(count_tree "$(source_of "$name")" "$destination")
    case "$counts" in
      *' 0 0 0 0') ;;
      *) fail "Project files changed since installation ($(describe_state $counts)); rerun with --force" ;;
    esac
  fi
  rm -rf -- "$destination"
  note "Removed $name from $destination"
}

case "${1:-}" in
  '' | -h | --help | help) usage ;;
  list)
    [ "$#" -eq 1 ] || fail 'Usage: manage.sh list'
    list_skills
    ;;
  status)
    [ "$#" -eq 2 ] || fail 'Usage: manage.sh status /absolute/project'
    project=$(require_project "$2")
    status_project "$project"
    ;;
  add)
    [ "$#" -eq 3 ] || fail 'Usage: manage.sh add /absolute/project skill-id'
    project=$(require_project "$2")
    add_skill "$project" "$3"
    ;;
  update)
    [ "$#" -eq 3 ] || fail 'Usage: manage.sh update /absolute/project skill-id [--force]'
    project=$(require_project "$2")
    update_skill "$project" "$3" "$force"
    ;;
  remove)
    [ "$#" -eq 3 ] || fail 'Usage: manage.sh remove /absolute/project skill-id [--force]'
    project=$(require_project "$2")
    remove_skill "$project" "$3" "$force"
    ;;
  *)
    usage >&2
    fail "Unknown command: $1"
    ;;
esac
