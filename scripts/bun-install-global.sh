#!/bin/bash
REPOSITORY_ROOT_DIR="$(cd $(dirname $0); pwd)/../.."
DEV_AND_DEPENDENCIES_PACKAGES=$(cat ${REPOSITORY_ROOT_DIR}/package.json | jq -r '[(.dependencies // {}), (.devDependencies // {})]
  | add | to_entries | map("\(.key)@\(.value)") | join(" ")')

bun add -g ${DEV_AND_DEPENDENCIES_PACKAGES}
# DEBUG CODE
# echo ${DEV_AND_DEPENDENCIES_PACKAGES}
