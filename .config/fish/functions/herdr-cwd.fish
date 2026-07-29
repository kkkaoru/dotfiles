function herdr-cwd --description "Open a Herdr workspace for the current directory path"
    argparse -i 's/sync' 'd/directory-session' -- $argv; or return 2

    if set -q _flag_sync; and set -q _flag_directory_session
        echo "hdr: --sync and --directory-session cannot be used together" >&2
        return 2
    end

    set -l real_pwd (pwd -P)
    set -l dir_name (basename "$real_pwd")
    set -l safe_dir (string replace -ra '[^A-Za-z0-9._-]+' '_' -- "$dir_name" | string sub -l 10)
    set -l path_hash (printf '%s' "$real_pwd" | cksum | string split ' ' | head -n 1 | string sub -l 8)
    set -l workspace_label "$safe_dir-$path_hash"

    # Keep the existing persistent, per-directory session as an explicit mode.
    if set -q _flag_directory_session
        command herdr --session "$workspace_label" $argv
        return $status
    end

    # Keep Herdr's normal CLI behavior when options or subcommands are supplied.
    if test (count $argv) -gt 0
        command herdr $argv
        return $status
    end

    # No option (or --sync) uses the shared default session and its path-based
    # workspaces, matching hdr's behavior before session-mode options existed.
    # The first client call can race the persistent server startup. Keep the
    # API work ahead of the TUI so the shared default session gets its path
    # workspace before the user attaches to it.
    set -l retry_limit 25
    set -l retry_delay 0.2
    set -l server_checked 0
    set -l server_status
    set -l server_running false
    set -l workspace_prepared 0
    set -l attempt 1
    set -l workspace_json
    set -l workspace_id
    set -l snapshot_json
    set -l initial_workspace_id

    while test $attempt -le $retry_limit
        set workspace_json (command herdr workspace list 2>/dev/null)
        if test $status -ne 0
            # Do not compete with an existing server. If the status probe says
            # it is absent, a background server is safe: a concurrent server
            # wins the socket race and this process simply keeps retrying.
            if test $server_checked -eq 0
                set server_checked 1
                set server_status (command herdr status server --json 2>/dev/null)
                set server_running (printf '%s\n' "$server_status" | command jq -r '.running // false' 2>/dev/null)
                if test "$server_running" != true
                    command herdr server </dev/null >/dev/null 2>&1 &
                    disown $last_pid 2>/dev/null; or true
                end
            end
        else
            set workspace_id (printf '%s\n' "$workspace_json" | command jq -r --arg label "$workspace_label" '.result.workspaces[]? | select(.label == $label) | .workspace_id // empty' | head -n 1)

            if test -n "$workspace_id"
                if command herdr workspace focus "$workspace_id" >/dev/null 2>&1
                    set workspace_prepared 1
                    break
                end
            else
                # A headless server may report an empty workspace list while
                # its initial pane is still becoming visible. Reuse that pane
                # when the snapshot is ready; otherwise create the requested
                # workspace directly. Both operations are retried below.
                set snapshot_json (command herdr api snapshot 2>/dev/null)
                if test $status -eq 0
                    set initial_workspace_id (printf '%s\n' "$snapshot_json" | command jq -r --arg cwd "$real_pwd" '.result.snapshot.panes[]? | select(.cwd == $cwd or .foreground_cwd == $cwd) | .workspace_id // empty' | head -n 1)

                    if test -n "$initial_workspace_id"
                        if command herdr workspace rename "$initial_workspace_id" "$workspace_label" >/dev/null 2>&1
                            if command herdr workspace focus "$initial_workspace_id" >/dev/null 2>&1
                                set workspace_prepared 1
                                break
                            end
                        end
                    end
                end

                # If the initial pane was not visible yet, or could not be
                # renamed/focused, make the path workspace explicitly. This
                # is the important first-run fallback: a transient snapshot
                # failure must not leave the first hdr invocation without a
                # workspace.
                if test $workspace_prepared -ne 1
                    if command herdr workspace create --cwd "$real_pwd" --label "$workspace_label" --focus >/dev/null 2>&1
                        set workspace_prepared 1
                        break
                    end
                end
            end
        end

        if test $attempt -lt $retry_limit
            sleep $retry_delay
        end
        set attempt (math $attempt + 1)
    end

    if test $workspace_prepared -ne 1
        echo "hdr: failed to prepare Herdr workspace '$workspace_label'" >&2
        return 1
    end

    command herdr
end
