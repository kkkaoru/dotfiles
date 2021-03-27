#!/bin/bash

mkdir -p ~/.ssh
if [ -f ~/.ssh/id_ecdsa ]; then
  echo "Exist key"
else
  ssh-keygen -t ecdsa -b 256
fi 