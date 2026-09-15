#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
helper="$repo/.agents/skills/project-skills/scripts/manage.sh"
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
export AGENT_SKILLS_STORAGE="$fixture/shared/.agents/skills-stroage"
mkdir -p "$AGENT_SKILLS_STORAGE/demo/references" "$fixture/project with spaces"
printf '%s\n' '---' 'name: demo' 'description: Test skill' '---' > "$AGENT_SKILLS_STORAGE/demo/SKILL.md"
printf 'reference\n' > "$AGENT_SKILLS_STORAGE/demo/references/example.md"
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
refuse() { if bash "$helper" "$@" > "$fixture/negative.log" 2>&1; then fail "Accepted invalid request: $*"; fi; }
[ "$(bash "$helper" list)" = demo ] || fail 'catalog'
bash "$helper" add "$fixture/project with spaces" demo
cmp "$AGENT_SKILLS_STORAGE/demo/SKILL.md" "$fixture/project with spaces/.agents/skills/demo/SKILL.md"
cmp "$AGENT_SKILLS_STORAGE/demo/references/example.md" "$fixture/project with spaces/.agents/skills/demo/references/example.md"
[ ! -L "$fixture/project with spaces/.agents/skills/demo" ] || fail 'expected portable copy'
printf 'local edit\n' > "$fixture/project with spaces/.agents/skills/demo/SKILL.md"
refuse add "$fixture/project with spaces" demo
[ "$(head -1 "$fixture/project with spaces/.agents/skills/demo/SKILL.md")" = 'local edit' ] || fail 'overwrote local edit'
refuse add relative demo
refuse add "$fixture/missing" demo
refuse add "$fixture/project with spaces" ../demo
refuse add "$fixture/project with spaces" unknown
refuse add "$fixture/shared" demo
refuse add "$fixture/project with spaces" '--force'
refuse list extra
refuse unknown
mkdir "$fixture/linked-project" "$fixture/linked-skills" "$fixture/target"
ln -s "$fixture/target" "$fixture/linked-project/.agents"
refuse add "$fixture/linked-project" demo
mkdir "$fixture/linked-skills/.agents"
ln -s "$fixture/target" "$fixture/linked-skills/.agents/skills"
refuse add "$fixture/linked-skills" demo
ln -s "$AGENT_SKILLS_STORAGE/demo" "$AGENT_SKILLS_STORAGE/alias"
refuse add "$fixture/project with spaces" alias
ln -s /missing "$AGENT_SKILLS_STORAGE/demo/references/link"
refuse add "$fixture/target" demo
[ ! -e "$fixture/target/.agents" ] || fail 'mutation before source validation'
printf 'project-skills tests passed\n'
