#!/bin/bash

asdf plugin-add nodejs
asdf install nodejs latest
asdf global nodejs latest
asdf plugin-add direnv
asdf plugin-add deno https://github.com/asdf-community/asdf-deno.git
asdf install deno latest
