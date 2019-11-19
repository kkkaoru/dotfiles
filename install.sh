#!/bin/bash

DOTPATH=$(cd $(dirname $0); pwd)

for f in .??*
do
  # NOTE: For Debug
  # echo "$DOTPATH/$f"
  [ "$f" = ".git" ] && continue
  [ "$f" = ".gitmodules" ] && continue
  ln -snfv "$DOTPATH/$f" "$HOME"/"$f"
done