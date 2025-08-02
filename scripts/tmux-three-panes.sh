#!/bin/bash

# Get current directory and create a session name based on it
CURRENT_DIR=$(pwd)
# Convert directory path to a valid session name
# Replace / with -, remove leading -, and replace dots with _
SESSION_NAME="three-panes-$(echo "$CURRENT_DIR" | sed 's/\//-/g' | sed 's/^-//' | sed 's/\./_/g')"

# Check if session already exists, if so attach to it
if tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
    tmux attach-session -t "$SESSION_NAME"
else
    # Create new tmux session using session config file
    tmux new-session -d -s "$SESSION_NAME" -c "$CURRENT_DIR" \; source-file ~/dotfiles/.tmux.session.conf
    # Attach to the session
    tmux attach-session -t "$SESSION_NAME"
fi