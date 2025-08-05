#!/bin/bash

# Parse arguments
SUFFIX="0"
RESET_MODE=false
RESET_TARGET=""
KILL_MODE=false
KILL_TARGET=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --reset|-r)
            RESET_MODE=true
            shift
            # Check if next argument is a number (target for reset)
            if [[ $# -gt 0 && $1 =~ ^[0-9]+$ ]]; then
                RESET_TARGET="$1"
                SUFFIX="$1"
                shift
            fi
            ;;
        --kill|-k)
            KILL_MODE=true
            shift
            # Check if next argument is "all" or a number
            if [[ $# -gt 0 ]]; then
                if [[ $1 == "all" ]]; then
                    KILL_TARGET="all"
                    shift
                elif [[ $1 =~ ^[0-9]+$ ]]; then
                    KILL_TARGET="$1"
                    shift
                else
                    echo "Error: --kill requires 'all' or a number argument"
                    echo "Usage: $0 [number] [--reset|-r [number]] [--kill|-k number|all]"
                    exit 1
                fi
            else
                echo "Error: --kill requires 'all' or a number argument"
                echo "Usage: $0 [number] [--reset|-r [number]] [--kill|-k number|all]"
                exit 1
            fi
            ;;
        [0-9]*)
            SUFFIX="$1"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [number] [--reset|-r [number]] [--kill|-k number]"
            exit 1
            ;;
    esac
done

# Get current directory and create a session name based on it
CURRENT_DIR=$(pwd)
# Convert directory path to a valid session name
# Replace / with -, remove leading -, and replace dots with _
SESSION_BASE="three-panes-$(echo "$CURRENT_DIR" | sed 's/\//-/g' | sed 's/^-//' | sed 's/\./_/g')"
SESSION_NAME="${SESSION_BASE}-${SUFFIX}"

# Handle kill mode
if [ "$KILL_MODE" = true ]; then
    if [ "$KILL_TARGET" = "all" ]; then
        # Kill all sessions matching the pattern for this directory
        tmux list-sessions -F "#{session_name}" 2>/dev/null | grep "^${SESSION_BASE}-" | while read -r session; do
            echo "Killing session: $session"
            tmux kill-session -t "$session"
        done
    else
        kill_session="${SESSION_BASE}-${KILL_TARGET}"
        if tmux has-session -t "$kill_session" 2>/dev/null; then
            echo "Killing session: $kill_session"
            tmux kill-session -t "$kill_session"
        else
            echo "Session not found: $kill_session"
        fi
    fi
    exit 0
fi

# Handle reset mode
if [ "$RESET_MODE" = true ]; then
    if [ -n "$RESET_TARGET" ]; then
        # Reset only the specified session
        target_session="${SESSION_BASE}-${RESET_TARGET}"
        if tmux has-session -t "$target_session" 2>/dev/null; then
            echo "Killing session: $target_session"
            tmux kill-session -t "$target_session"
        fi
    else
        # Kill all sessions matching the pattern for this directory
        tmux list-sessions -F "#{session_name}" 2>/dev/null | grep "^${SESSION_BASE}-" | while read -r session; do
            echo "Killing session: $session"
            tmux kill-session -t "$session"
        done
    fi
    # Create new session after reset
    tmux new-session -d -s "$SESSION_NAME" -c "$CURRENT_DIR" \; source-file ~/dotfiles/.tmux.session.conf
    tmux attach-session -t "$SESSION_NAME"
elif tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
    # Check if session already exists, if so attach to it
    tmux attach-session -t "$SESSION_NAME"
else
    # Create new tmux session using session config file
    tmux new-session -d -s "$SESSION_NAME" -c "$CURRENT_DIR" \; source-file ~/dotfiles/.tmux.session.conf
    # Attach to the session
    tmux attach-session -t "$SESSION_NAME"
fi