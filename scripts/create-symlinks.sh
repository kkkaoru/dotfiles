#!/bin/bash

SCRIPT_DIR=$(cd $(dirname $0); pwd)

for f in .??*
do
  [ "$f" = ".git" ] && continue
  [ "$f" = ".tool-versions" ] && continue
  [ "$f" = ".cursor" ] && continue
  [ "$f" = ".npmrc" ] && continue
  [ -L "${HOME}/${f}" ] && continue
  # NOTE: For Debug
  # echo "$SCRIPT_DIR/$f"
  ln -snfv "$SCRIPT_DIR/$f" "$HOME"
done
