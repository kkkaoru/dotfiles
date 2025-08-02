#!/bin/bash

# Check if session already exists, if so attach to it
if tmux has-session -t three-panes 2>/dev/null; then
    tmux attach-session -t three-panes
else
    # Create new tmux session using session config file
    tmux new-session -d -s three-panes -c /Users/kaoru/dotfiles \; source-file /Users/kaoru/dotfiles/.tmux.session.conf
    # Attach to the session
    tmux attach-session -t three-panes
fi