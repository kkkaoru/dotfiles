#!/bin/bash

DOTPATH=$(cd $(dirname $0); pwd)

for f in .??*
do
  [ "$f" = ".git" ] && continue
  [ -L "${HOME}/${f}" ] && continue
  # NOTE: For Debug
  # echo "$DOTPATH/$f"
  ln -snfv "$DOTPATH/$f" "$HOME"
done
